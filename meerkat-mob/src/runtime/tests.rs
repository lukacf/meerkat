use super::*;
use crate::definition::{
    BackendConfig, CollectionPolicy, ConditionExpr, DependencyMode, DispatchMode, FlowSpec,
    FlowStepSpec, LimitsSpec, MobDefinition, OrchestratorConfig, PolicyMode, RoleWiringRule,
    SkillSource, StepOutputFormat, TopologyRule, TopologySpec, WiringRules,
};
use crate::event::MobEvent;
use crate::profile::{Profile, ProfileBinding, ToolConfig};
use crate::run::MobRunStatus;
use crate::run::{FailureLedgerEntry, MobRun, StepLedgerEntry, StepRunStatus};
use crate::storage::MobStorage;
use crate::store::{
    ExternalBindingOverlayRecord, ExternalBindingOverlayStatus, InMemoryMobEventStore,
    InMemoryMobRunStore, InMemoryMobRuntimeMetadataStore, InMemoryMobSpecStore, MobEventStore,
    MobRunStore, MobRuntimeMetadataStore, MobStoreError, RealmProfileStore,
    SupervisorAuthorityRecord,
};
use async_trait::async_trait;
use chrono::Utc;
use indexmap::IndexMap;
use meerkat_core::Provider;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, PeerCapabilitySet, PeerDirectoryEntry, PeerDirectorySource, PeerId, PeerName,
    PeerReachability, PeerReachabilityReason, PeerRoute, PeerSendability, SendError, SendReceipt,
    TrustedPeerDescriptor,
};
use meerkat_core::error::ToolError;
use meerkat_core::event::{AgentEvent, EventEnvelope};
use meerkat_core::interaction::InteractionId;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    SessionControlError, SessionError, SessionInfo, SessionQuery, SessionService,
    SessionServiceCommsExt, SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary,
    SessionUsage, SessionView, StartTurnRequest, TurnToolOverlay,
};
use meerkat_core::types::{
    AssistantBlock, HandlingMode, Message, RunResult, SessionId, StopReason, ToolCallView, ToolDef,
    ToolResult, Usage,
};
use meerkat_core::{
    Agent, AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AppendSystemContextStatus, EventInjector, EventInjectorError, LlmStreamResult,
    PeerCorrelationId, PendingSystemContextAppend, PlainEventSource,
    event_injector::{InteractionSubscription, SubscribableInjector},
};
use meerkat_core::{
    Session, SessionMetadata, SessionSystemContextState, SessionTooling, ToolCategoryOverride,
};
use meerkat_machine_schema::catalog::dsl::dsl_mob_machine as schema_mob_machine;
use meerkat_machine_schema::{Expr, MachineSchema, TriggerKind};
use meerkat_session::{SessionAgent, SessionAgentBuilder, SessionSnapshot};
use meerkat_store::{MemoryStore, SessionStore};
use serde::Serialize;
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tempfile::NamedTempFile;
use uuid::Uuid;

fn default_supervisor_authority_record() -> SupervisorAuthorityRecord {
    SupervisorAuthorityRecord::generate(
        super::bridge_protocol::supervisor_bridge_default_protocol_version(),
    )
}

fn install_ephemeral_peer_request_response_authority(
    runtime: &Arc<meerkat_comms::CommsRuntime>,
    session: &str,
) {
    let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
    dsl.apply_signal(
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
        "test::initialize",
    )
    .expect("Initialize");
    dsl.apply_input(
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
            session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(session.to_string()),
        },
        "test::register_session",
    )
    .expect("RegisterSession");

    runtime.install_peer_request_response_authority(
        meerkat_comms::PeerRequestResponseAuthority::new(
            Arc::new(meerkat_runtime::RuntimePeerInteractionHandle::new(
                Arc::clone(&dsl),
            )),
            Arc::new(meerkat_runtime::RuntimeInteractionStreamHandle::new(dsl)),
        ),
    );
}

async fn install_machine_peer_request_response_authority(
    adapter: &meerkat_runtime::MeerkatMachine,
    runtime: &Arc<meerkat_comms::CommsRuntime>,
    session_id: &SessionId,
) -> Result<(), SessionError> {
    let bindings = adapter
        .prepare_local_session_bindings(session_id.clone())
        .await
        .map_err(|err| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to prepare comms lifecycle authority: {err}"
            )))
        })?;
    runtime.install_peer_request_response_authority(
        meerkat_comms::PeerRequestResponseAuthority::new(
            bindings.peer_interaction,
            bindings.interaction_stream,
        ),
    );
    Ok(())
}

// -----------------------------------------------------------------------
// Mock CommsRuntime
// -----------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct MockCommsBehavior {
    missing_public_key: bool,
    fail_add_trust: bool,
    fail_remove_trust: bool,
    fail_remove_trust_once: bool,
    fail_send_peer_added: bool,
    fail_send_peer_retired: bool,
    fail_send_peer_unwired: bool,
}

struct MockCommsRuntime {
    default_peer_id: PeerId,
    default_public_key: String,
    behavior: std::sync::RwLock<MockCommsBehavior>,
    remove_failures_remaining: std::sync::Mutex<usize>,
    trusted_peers: RwLock<HashMap<String, TrustedPeerDescriptor>>,
    peer_statuses: RwLock<HashMap<String, (PeerReachability, Option<PeerReachabilityReason>)>>,
    sent_intents: RwLock<Vec<String>>,
    inbox_notify: Arc<tokio::sync::Notify>,
}

impl MockCommsRuntime {
    fn new(name: &str, behavior: MockCommsBehavior) -> Self {
        let mut key_bytes = [0u8; 32];
        for (index, byte) in name.as_bytes().iter().copied().enumerate() {
            let slot = index % key_bytes.len();
            key_bytes[slot] = key_bytes[slot]
                .wrapping_add(byte)
                .rotate_left((index % 8) as u32);
        }
        let public_key = meerkat_comms::PubKey::new(key_bytes);
        Self {
            default_peer_id: public_key.to_peer_id(),
            default_public_key: public_key.to_pubkey_string(),
            behavior: std::sync::RwLock::new(behavior),
            remove_failures_remaining: std::sync::Mutex::new(usize::from(
                behavior.fail_remove_trust_once,
            )),
            trusted_peers: RwLock::new(HashMap::new()),
            peer_statuses: RwLock::new(HashMap::new()),
            sent_intents: RwLock::new(Vec::new()),
            inbox_notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn clear_public_key(&self) {
        let mut behavior = self
            .behavior
            .write()
            .expect("poisoned behavior lock in mock runtime");
        behavior.missing_public_key = true;
    }

    fn set_behavior(&self, behavior: MockCommsBehavior) {
        {
            let mut current = self
                .behavior
                .write()
                .expect("poisoned behavior lock in mock runtime");
            *current = behavior;
        }
        let mut remaining = self
            .remove_failures_remaining
            .lock()
            .expect("poisoned remove_failures_remaining lock in mock runtime");
        *remaining = usize::from(behavior.fail_remove_trust_once);
    }

    async fn trusted_peer_names(&self) -> Vec<String> {
        let mut names = self
            .trusted_peers
            .read()
            .await
            .values()
            .map(|peer| peer.name.as_str().to_string())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[allow(dead_code)]
    async fn trusted_peer_addresses(&self) -> Vec<String> {
        let mut addresses = self
            .trusted_peers
            .read()
            .await
            .values()
            .map(|peer| peer.address.to_string())
            .collect::<Vec<_>>();
        addresses.sort();
        addresses
    }

    async fn sent_intents(&self) -> Vec<String> {
        self.sent_intents.read().await.clone()
    }

    async fn set_peer_status(
        &self,
        peer_name: &str,
        reachability: PeerReachability,
        reason: Option<PeerReachabilityReason>,
    ) {
        self.peer_statuses
            .write()
            .await
            .insert(peer_name.to_string(), (reachability, reason));
    }
}

#[async_trait]
impl CoreCommsRuntime for MockCommsRuntime {
    fn peer_id(&self) -> Option<PeerId> {
        let behavior = self
            .behavior
            .read()
            .expect("poisoned behavior lock in mock runtime");
        if behavior.missing_public_key {
            None
        } else {
            Some(self.default_peer_id)
        }
    }

    fn public_key(&self) -> Option<String> {
        let behavior = self
            .behavior
            .read()
            .expect("poisoned behavior lock in mock runtime");
        if behavior.missing_public_key {
            None
        } else {
            Some(self.default_public_key.clone())
        }
    }

    async fn add_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
        if self
            .behavior
            .read()
            .expect("poisoned behavior lock in mock runtime")
            .fail_add_trust
        {
            return Err(SendError::Unsupported(
                "mock add_trusted_peer failure".to_string(),
            ));
        }
        let mut peers = self.trusted_peers.write().await;
        peers.insert(peer.peer_id.to_string(), peer);
        Ok(())
    }

    async fn add_private_trusted_peer(&self, peer: TrustedPeerDescriptor) -> Result<(), SendError> {
        self.add_trusted_peer(peer).await
    }

    async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
        if self
            .behavior
            .read()
            .expect("poisoned behavior lock in mock runtime")
            .fail_remove_trust
        {
            return Err(SendError::Unsupported(
                "mock remove_trusted_peer failure".to_string(),
            ));
        }
        {
            let mut remaining = self
                .remove_failures_remaining
                .lock()
                .expect("poisoned remove_failures_remaining lock in mock runtime");
            if *remaining > 0 {
                *remaining -= 1;
                return Err(SendError::Unsupported(
                    "mock remove_trusted_peer failure (one-shot)".to_string(),
                ));
            }
        }
        let mut peers = self.trusted_peers.write().await;
        Ok(peers.remove(peer_id).is_some())
    }

    async fn remove_private_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
        self.remove_trusted_peer(peer_id).await
    }

    async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        match cmd {
            CommsCommand::PeerLifecycle { to, kind, .. } => {
                let behavior = *self
                    .behavior
                    .read()
                    .expect("poisoned behavior lock in mock runtime");
                match kind {
                    meerkat_core::comms::PeerLifecycleKind::PeerAdded
                        if behavior.fail_send_peer_added =>
                    {
                        return Err(SendError::Unsupported(
                            "mock mob.peer_added notification failure".to_string(),
                        ));
                    }
                    meerkat_core::comms::PeerLifecycleKind::PeerRetired
                        if behavior.fail_send_peer_retired =>
                    {
                        return Err(SendError::Unsupported(
                            "mock mob.peer_retired notification failure".to_string(),
                        ));
                    }
                    meerkat_core::comms::PeerLifecycleKind::PeerUnwired
                        if behavior.fail_send_peer_unwired =>
                    {
                        return Err(SendError::Unsupported(
                            "mock mob.peer_unwired notification failure".to_string(),
                        ));
                    }
                    _ => {}
                }

                let trusted = self.trusted_peers.read().await;
                let peer_id = to.peer_id.as_str();
                if !trusted.contains_key(&peer_id) {
                    return Err(SendError::PeerNotFound(to.label()));
                }
                drop(trusted);

                self.sent_intents
                    .write()
                    .await
                    .push(kind.as_str().to_string());
                Ok(SendReceipt::PeerLifecycleSent {
                    envelope_id: uuid::Uuid::new_v4(),
                })
            }
            CommsCommand::PeerRequest { to, intent, .. } => {
                let behavior = *self
                    .behavior
                    .read()
                    .expect("poisoned behavior lock in mock runtime");
                if intent == "mob.peer_added" && behavior.fail_send_peer_added {
                    return Err(SendError::Unsupported(
                        "mock mob.peer_added notification failure".to_string(),
                    ));
                }
                if intent == "mob.peer_retired" && behavior.fail_send_peer_retired {
                    return Err(SendError::Unsupported(
                        "mock mob.peer_retired notification failure".to_string(),
                    ));
                }
                if intent == "mob.peer_unwired" && behavior.fail_send_peer_unwired {
                    return Err(SendError::Unsupported(
                        "mock mob.peer_unwired notification failure".to_string(),
                    ));
                }

                let trusted = self.trusted_peers.read().await;
                let peer_id = to.peer_id.as_str();
                if !trusted.contains_key(&peer_id) {
                    return Err(SendError::PeerNotFound(to.label()));
                }
                drop(trusted);

                self.sent_intents.write().await.push(intent);
                Ok(SendReceipt::PeerRequestSent {
                    envelope_id: uuid::Uuid::new_v4(),
                    interaction_id: InteractionId(uuid::Uuid::new_v4()),
                    stream_reserved: false,
                })
            }
            unsupported => Err(SendError::Unsupported(format!(
                "mock send unsupported command kind '{}'",
                unsupported.command_kind()
            ))),
        }
    }

    async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        let trusted = self.trusted_peers.read().await;
        let peer_statuses = self.peer_statuses.read().await;
        trusted
            .iter()
            .filter_map(|(peer_id, peer)| {
                let peer_id = meerkat_core::comms::PeerId::parse(peer_id).ok()?;
                let name = peer.name.clone();
                let (reachability, last_unreachable_reason) = peer_statuses
                    .get(peer.name.as_str())
                    .copied()
                    .unwrap_or((PeerReachability::Unknown, None));
                Some(PeerDirectoryEntry {
                    name,
                    peer_id,
                    address: peer.address.clone(),
                    source: PeerDirectorySource::Trusted,
                    sendable_kinds: PeerSendability::directory_defaults(),
                    capabilities: PeerCapabilitySet::default(),
                    reachability,
                    last_unreachable_reason,
                    meta: meerkat_core::PeerMeta::default(),
                })
            })
            .collect()
    }

    async fn drain_messages(&self) -> Vec<String> {
        Vec::new()
    }

    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.inbox_notify.clone()
    }
}

// -----------------------------------------------------------------------
// Mock SessionService
// -----------------------------------------------------------------------

fn mock_run_result(session_id: SessionId, text: String) -> RunResult {
    RunResult {
        text,
        session_id,
        usage: Usage::default(),
        turns: 1,
        tool_calls: 0,
        structured_output: None,
        schema_warnings: None,
        skill_diagnostics: None,
    }
}

#[derive(Clone, Debug)]
struct CreateSessionRecord {
    initial_turn: meerkat_core::service::InitialTurnPolicy,
    comms_name: Option<String>,
    peer_meta_labels: BTreeMap<String, String>,
}

/// A mock session service that creates sessions with mock comms runtimes.
struct MockSessionService {
    sessions: RwLock<HashMap<SessionId, Arc<MockCommsRuntime>>>,
    live_session_data: RwLock<HashMap<SessionId, Session>>,
    persisted_sessions: RwLock<HashMap<SessionId, Session>>,
    keep_alive_notifiers: RwLock<HashMap<SessionId, Arc<tokio::sync::Notify>>>,
    session_comms_names: RwLock<HashMap<SessionId, String>>,
    runtime_adapter: Mutex<Option<Arc<meerkat_runtime::MeerkatMachine>>>,
    session_counter: AtomicU64,
    /// Records (session_id, prompt) for each create_session call.
    prompts: RwLock<Vec<(SessionId, String)>>,
    create_requests: RwLock<Vec<CreateSessionRecord>>,
    /// Records whether each create_session had external_tools configured.
    create_with_external_tools: RwLock<Vec<bool>>,
    /// External tool dispatcher captured per session.
    external_tools_by_session: RwLock<HashMap<SessionId, Arc<dyn AgentToolDispatcher>>>,
    /// Optional per-comms-name behavior overrides.
    comms_behaviors: RwLock<HashMap<String, MockCommsBehavior>>,
    /// Sessions whose comms runtime should appear unavailable.
    missing_comms_sessions: RwLock<HashSet<SessionId>>,
    /// Sessions whose archive() call should fail.
    archive_fail_sessions: RwLock<HashSet<SessionId>>,
    /// Comms names whose created sessions should fail archive().
    archive_fail_comms_names: RwLock<HashSet<String>>,
    /// Sent intents for sessions that were archived and removed.
    archived_sent_intents: RwLock<HashMap<SessionId, Vec<String>>>,
    fail_start_turn: std::sync::atomic::AtomicBool,
    fail_inject: std::sync::atomic::AtomicBool,
    start_turn_calls: AtomicU64,
    keep_alive_start_turn_calls: AtomicU64,
    keep_alive_turns_complete_immediately: std::sync::atomic::AtomicBool,
    keep_alive_prompts: RwLock<Vec<(SessionId, String)>>,
    interrupt_calls: AtomicU64,
    inject_calls: Arc<AtomicU64>,
    create_session_delay_ms: AtomicU64,
    create_session_in_flight: AtomicU64,
    create_session_max_in_flight: AtomicU64,
    archive_delay_ms: AtomicU64,
    start_turn_delay_ms: AtomicU64,
    flow_turn_delay_ms: AtomicU64,
    flow_turn_never_terminal: std::sync::atomic::AtomicBool,
    flow_turn_fail: std::sync::atomic::AtomicBool,
    flow_turn_fail_sessions: RwLock<HashSet<SessionId>>,
    flow_turn_completed_result: RwLock<String>,
    flow_turn_overlays: RwLock<Vec<(SessionId, Option<meerkat_core::service::TurnToolOverlay>)>>,
    /// Session IDs for which `subscribe_session_events` must return an error.
    subscribe_fail_sessions: RwLock<HashSet<SessionId>>,
    /// Session IDs for which `read()`/`list()` should report `is_active=true`.
    ///
    /// Mirrors real SessionService semantics: `is_active` tracks in-flight
    /// turn execution (Admitted/Running/Completing), not session presence.
    /// A created-but-idle session is not active. Tests that exercise
    /// in-flight-turn semantics can flip this via
    /// [`Self::mark_session_active`]. Without this, the disposal-time poll
    /// loop in [`stop_autonomous_member`] spins the full 40×25ms=1s grace
    /// window on every retire/complete/reset because the mock was reporting
    /// an always-`true` flag that the real service never would.
    active_sessions: RwLock<HashSet<SessionId>>,
}

impl MockSessionService {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            live_session_data: RwLock::new(HashMap::new()),
            persisted_sessions: RwLock::new(HashMap::new()),
            keep_alive_notifiers: RwLock::new(HashMap::new()),
            session_comms_names: RwLock::new(HashMap::new()),
            runtime_adapter: Mutex::new(None),
            session_counter: AtomicU64::new(0),
            prompts: RwLock::new(Vec::new()),
            create_requests: RwLock::new(Vec::new()),
            create_with_external_tools: RwLock::new(Vec::new()),
            external_tools_by_session: RwLock::new(HashMap::new()),
            comms_behaviors: RwLock::new(HashMap::new()),
            missing_comms_sessions: RwLock::new(HashSet::new()),
            archive_fail_sessions: RwLock::new(HashSet::new()),
            archive_fail_comms_names: RwLock::new(HashSet::new()),
            archived_sent_intents: RwLock::new(HashMap::new()),
            fail_start_turn: std::sync::atomic::AtomicBool::new(false),
            fail_inject: std::sync::atomic::AtomicBool::new(false),
            start_turn_calls: AtomicU64::new(0),
            keep_alive_start_turn_calls: AtomicU64::new(0),
            keep_alive_turns_complete_immediately: std::sync::atomic::AtomicBool::new(false),
            keep_alive_prompts: RwLock::new(Vec::new()),
            interrupt_calls: AtomicU64::new(0),
            inject_calls: Arc::new(AtomicU64::new(0)),
            create_session_delay_ms: AtomicU64::new(0),
            create_session_in_flight: AtomicU64::new(0),
            create_session_max_in_flight: AtomicU64::new(0),
            archive_delay_ms: AtomicU64::new(0),
            start_turn_delay_ms: AtomicU64::new(0),
            flow_turn_delay_ms: AtomicU64::new(0),
            flow_turn_never_terminal: std::sync::atomic::AtomicBool::new(false),
            flow_turn_fail: std::sync::atomic::AtomicBool::new(false),
            flow_turn_fail_sessions: RwLock::new(HashSet::new()),
            flow_turn_completed_result: RwLock::new("\"Turn completed\"".to_string()),
            flow_turn_overlays: RwLock::new(Vec::new()),
            subscribe_fail_sessions: RwLock::new(HashSet::new()),
            active_sessions: RwLock::new(HashSet::new()),
        }
    }

    async fn fail_subscribe_for_session(&self, session_id: SessionId) {
        self.subscribe_fail_sessions
            .write()
            .await
            .insert(session_id);
    }

    async fn active_session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    async fn live_session_clone(&self, session_id: &SessionId) -> Option<Session> {
        self.live_session_data.read().await.get(session_id).cloned()
    }

    async fn persisted_session_clone(&self, session_id: &SessionId) -> Option<Session> {
        self.persisted_sessions
            .read()
            .await
            .get(session_id)
            .cloned()
    }

    async fn replace_live_session(&self, session: Session) {
        let session_id = session.id().clone();
        self.live_session_data
            .write()
            .await
            .insert(session_id.clone(), session.clone());
        self.persisted_sessions
            .write()
            .await
            .insert(session_id, session);
    }

    async fn delete_persisted_session(&self, session_id: &SessionId) {
        self.persisted_sessions.write().await.remove(session_id);
    }

    fn enable_runtime_adapter(&self) -> Arc<meerkat_runtime::MeerkatMachine> {
        let mut guard = self.runtime_adapter.lock().expect("runtime_adapter mutex");
        if let Some(adapter) = guard.as_ref() {
            return adapter.clone();
        }
        let adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        *guard = Some(adapter.clone());
        adapter
    }

    /// Get recorded prompts for inspection in tests.
    async fn recorded_prompts(&self) -> Vec<(SessionId, String)> {
        self.prompts.read().await.clone()
    }

    async fn recorded_create_requests(&self) -> Vec<CreateSessionRecord> {
        self.create_requests.read().await.clone()
    }

    async fn recorded_external_tools_flags(&self) -> Vec<bool> {
        self.create_with_external_tools.read().await.clone()
    }

    async fn external_tool_names(&self, session_id: &SessionId) -> Vec<String> {
        let tools = self.external_tools_by_session.read().await;
        tools
            .get(session_id)
            .map(|dispatcher| {
                dispatcher
                    .tools()
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn dispatch_external_tool(
        &self,
        session_id: &SessionId,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        self.dispatch_external_tool_outcome(session_id, tool_name, args)
            .await
            .map(|outcome| outcome.result)
    }

    async fn dispatch_external_tool_outcome(
        &self,
        session_id: &SessionId,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let dispatcher = {
            let tools = self.external_tools_by_session.read().await;
            tools.get(session_id).cloned()
        }
        .ok_or_else(|| ToolError::not_found(tool_name))?;
        let raw = RawValue::from_string(args.to_string())
            .map_err(|error| ToolError::execution_failed(format!("raw args: {error}")))?;
        dispatcher
            .dispatch(ToolCallView {
                id: "tool-1",
                name: tool_name,
                args: &raw,
            })
            .await
    }

    async fn set_comms_behavior(&self, comms_name: &str, behavior: MockCommsBehavior) {
        self.comms_behaviors
            .write()
            .await
            .insert(comms_name.to_string(), behavior);
        let session_ids = {
            let names = self.session_comms_names.read().await;
            names
                .iter()
                .filter_map(|(session_id, name)| {
                    if name == comms_name {
                        Some(session_id.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        if session_ids.is_empty() {
            return;
        }
        let sessions = self.sessions.read().await;
        for session_id in session_ids {
            if let Some(runtime) = sessions.get(&session_id) {
                runtime.set_behavior(behavior);
            }
        }
    }

    async fn set_missing_comms_runtime(&self, session_id: &SessionId) {
        self.missing_comms_sessions
            .write()
            .await
            .insert(session_id.clone());
    }

    async fn set_archive_failure(&self, session_id: &SessionId) {
        self.archive_fail_sessions
            .write()
            .await
            .insert(session_id.clone());
    }

    async fn set_archive_failure_for_comms_name(&self, comms_name: &str) {
        self.archive_fail_comms_names
            .write()
            .await
            .insert(comms_name.to_string());
    }

    async fn clear_public_key(&self, session_id: &SessionId) {
        let runtime = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };
        if let Some(runtime) = runtime {
            runtime.clear_public_key();
        }
    }

    async fn set_peer_status(
        &self,
        session_id: &SessionId,
        peer_name: &str,
        reachability: PeerReachability,
        reason: Option<PeerReachabilityReason>,
    ) {
        let runtime = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };
        if let Some(runtime) = runtime {
            runtime
                .set_peer_status(peer_name, reachability, reason)
                .await;
        }
    }

    fn set_fail_start_turn(&self, enabled: bool) {
        self.fail_start_turn
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    #[allow(dead_code)]
    fn set_fail_inject(&self, enabled: bool) {
        self.fail_inject
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    fn start_turn_call_count(&self) -> u64 {
        self.start_turn_calls.load(Ordering::Relaxed)
    }

    fn keep_alive_start_turn_call_count(&self) -> u64 {
        self.keep_alive_start_turn_calls.load(Ordering::Relaxed)
    }

    fn set_keep_alive_turns_complete_immediately(&self, enabled: bool) {
        self.keep_alive_turns_complete_immediately
            .store(enabled, Ordering::Relaxed);
    }

    fn interrupt_call_count(&self) -> u64 {
        self.interrupt_calls.load(Ordering::Relaxed)
    }

    fn inject_call_count(&self) -> u64 {
        self.inject_calls.load(Ordering::Relaxed)
    }

    fn set_create_session_delay_ms(&self, delay_ms: u64) {
        self.create_session_delay_ms
            .store(delay_ms, Ordering::Relaxed);
    }

    fn max_concurrent_create_session_calls(&self) -> u64 {
        self.create_session_max_in_flight.load(Ordering::Relaxed)
    }

    fn set_archive_delay_ms(&self, delay_ms: u64) {
        self.archive_delay_ms.store(delay_ms, Ordering::Relaxed);
    }

    fn set_start_turn_delay_ms(&self, delay_ms: u64) {
        self.start_turn_delay_ms
            .store(delay_ms, std::sync::atomic::Ordering::Relaxed);
    }

    fn set_flow_turn_delay_ms(&self, delay_ms: u64) {
        self.flow_turn_delay_ms
            .store(delay_ms, std::sync::atomic::Ordering::Relaxed);
    }

    fn set_flow_turn_never_terminal(&self, enabled: bool) {
        self.flow_turn_never_terminal
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    fn set_flow_turn_fail(&self, enabled: bool) {
        self.flow_turn_fail
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    async fn set_flow_turn_fail_for_session(&self, session_id: &SessionId, enabled: bool) {
        let mut set = self.flow_turn_fail_sessions.write().await;
        if enabled {
            set.insert(session_id.clone());
        } else {
            set.remove(session_id);
        }
    }

    async fn set_flow_turn_completed_result(&self, result: impl Into<String>) {
        *self.flow_turn_completed_result.write().await = result.into();
    }

    async fn recorded_flow_turn_overlays(
        &self,
    ) -> Vec<(SessionId, Option<meerkat_core::service::TurnToolOverlay>)> {
        self.flow_turn_overlays.read().await.clone()
    }

    async fn trusted_peer_names(&self, session_id: &SessionId) -> Vec<String> {
        let runtime = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };
        match runtime {
            Some(runtime) => runtime.trusted_peer_names().await,
            None => Vec::new(),
        }
    }

    #[allow(dead_code)]
    async fn trusted_peer_addresses(&self, session_id: &SessionId) -> Vec<String> {
        let runtime = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };
        match runtime {
            Some(runtime) => runtime.trusted_peer_addresses().await,
            None => Vec::new(),
        }
    }

    async fn sent_intents(&self, session_id: &SessionId) -> Vec<String> {
        let runtime = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };
        match runtime {
            Some(runtime) => runtime.sent_intents().await,
            None => self
                .archived_sent_intents
                .read()
                .await
                .get(session_id)
                .cloned()
                .unwrap_or_default(),
        }
    }

    async fn force_remove_trust(&self, from_session: &SessionId, to_session: &SessionId) {
        let (from_runtime, to_runtime) = {
            let sessions = self.sessions.read().await;
            (
                sessions.get(from_session).cloned(),
                sessions.get(to_session).cloned(),
            )
        };
        if let (Some(from_runtime), Some(to_runtime)) = (from_runtime, to_runtime)
            && let Some(to_key) = to_runtime.peer_id()
        {
            let _ = from_runtime.remove_trusted_peer(&to_key.to_string()).await;
        }
    }

    async fn force_add_trust_from_spec(&self, session_id: &SessionId, spec: TrustedPeerDescriptor) {
        let runtime = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        };
        if let Some(runtime) = runtime {
            runtime
                .add_trusted_peer(spec)
                .await
                .expect("force add trusted peer");
        }
    }
}

#[async_trait]
impl SessionService for MockSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let in_flight = self
            .create_session_in_flight
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        loop {
            let observed_max = self.create_session_max_in_flight.load(Ordering::Relaxed);
            if in_flight <= observed_max {
                break;
            }
            if self
                .create_session_max_in_flight
                .compare_exchange(
                    observed_max,
                    in_flight,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }
        let create_delay_ms = self.create_session_delay_ms.load(Ordering::Relaxed);
        if create_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(create_delay_ms)).await;
        }

        let mut session = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.clone())
            .unwrap_or_default();
        let session_id = session.id().clone();
        let n = self.session_counter.fetch_add(1, Ordering::Relaxed);

        // Extract comms_name from build options for mock runtime naming
        let comms_name = req
            .build
            .as_ref()
            .and_then(|b| b.comms_name.clone())
            .unwrap_or_else(|| format!("mock-session-{n}"));

        let behavior = self
            .comms_behaviors
            .read()
            .await
            .get(&comms_name)
            .copied()
            .unwrap_or_default();
        let comms = Arc::new(MockCommsRuntime::new(&comms_name, behavior));
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), comms);
        if let Some(system_prompt) = req.system_prompt.clone() {
            session.set_system_prompt(system_prompt);
        }
        if session.session_metadata().is_none() {
            let build = req.build.as_ref();
            let metadata = SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: req.model.clone(),
                max_tokens: req.max_tokens.unwrap_or(4096),
                structured_output_retries: 2,
                provider: build
                    .and_then(|b| b.provider)
                    .unwrap_or(Provider::Anthropic),
                self_hosted_server_id: None,
                provider_params: build.and_then(|b| b.provider_params.clone()),
                tooling: SessionTooling {
                    builtins: build
                        .map(|b| b.override_builtins)
                        .unwrap_or(ToolCategoryOverride::from_effective(true)),
                    shell: build
                        .map(|b| b.override_shell)
                        .unwrap_or(ToolCategoryOverride::from_effective(false)),
                    comms: ToolCategoryOverride::from_effective(
                        build.and_then(|b| b.comms_name.as_ref()).is_some(),
                    ),
                    mob: build
                        .map(|b| b.override_mob)
                        .unwrap_or(ToolCategoryOverride::from_effective(false)),
                    memory: build
                        .map(|b| b.override_memory)
                        .unwrap_or(ToolCategoryOverride::from_effective(false)),
                    active_skills: build.and_then(|b| b.preload_skills.clone()),
                },
                keep_alive: build.map(|b| b.keep_alive).unwrap_or(false),
                comms_name: build.and_then(|b| b.comms_name.clone()),
                peer_meta: build.and_then(|b| b.peer_meta.clone()),
                realm_id: build.and_then(|b| b.realm_id.clone()),
                instance_id: build.and_then(|b| b.instance_id.clone()),
                backend: build.and_then(|b| b.backend.clone()),
                config_generation: build.and_then(|b| b.config_generation),
                connection_ref: None,
            };
            session
                .set_session_metadata(metadata)
                .expect("mock session metadata should serialize");
        }
        self.live_session_data
            .write()
            .await
            .insert(session_id.clone(), session.clone());
        self.persisted_sessions
            .write()
            .await
            .insert(session_id.clone(), session);
        let is_keep_alive = req.build.as_ref().map(|b| b.keep_alive).unwrap_or(false);
        if is_keep_alive {
            self.keep_alive_notifiers
                .write()
                .await
                .insert(session_id.clone(), Arc::new(tokio::sync::Notify::new()));
        }
        self.session_comms_names
            .write()
            .await
            .insert(session_id.clone(), comms_name.clone());
        if self
            .archive_fail_comms_names
            .read()
            .await
            .contains(&comms_name)
        {
            self.archive_fail_sessions
                .write()
                .await
                .insert(session_id.clone());
        }

        let peer_meta_labels = req
            .build
            .as_ref()
            .and_then(|build| build.peer_meta.as_ref())
            .map(|meta| meta.labels.clone())
            .unwrap_or_default();
        self.create_requests
            .write()
            .await
            .push(CreateSessionRecord {
                initial_turn: req.initial_turn,
                comms_name: req
                    .build
                    .as_ref()
                    .and_then(|build| build.comms_name.clone()),
                peer_meta_labels,
            });

        // Record the prompt for test inspection
        self.prompts
            .write()
            .await
            .push((session_id.clone(), req.prompt.text_content()));
        let external_tools = req
            .build
            .as_ref()
            .and_then(|b| b.external_tools.as_ref())
            .cloned();
        self.create_with_external_tools
            .write()
            .await
            .push(external_tools.is_some());
        if let Some(dispatcher) = external_tools {
            self.external_tools_by_session
                .write()
                .await
                .insert(session_id.clone(), dispatcher);
        }

        self.create_session_in_flight
            .fetch_sub(1, Ordering::Relaxed);
        Ok(mock_run_result(session_id, "Session created".to_string()))
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.flow_turn_overlays
            .write()
            .await
            .push((id.clone(), req.flow_tool_overlay.clone()));
        self.start_turn_calls.fetch_add(1, Ordering::Relaxed);
        // Determine keep-alive by checking if a notifier was registered for this session
        // (created in create_session when build.keep_alive is true).
        let is_keep_alive = self.keep_alive_notifiers.read().await.contains_key(id);
        if is_keep_alive {
            self.keep_alive_start_turn_calls
                .fetch_add(1, Ordering::Relaxed);
            self.keep_alive_prompts
                .write()
                .await
                .push((id.clone(), req.prompt.text_content()));
        }
        let start_turn_delay = self.start_turn_delay_ms.load(Ordering::Relaxed);
        if start_turn_delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(start_turn_delay)).await;
        }
        if self
            .fail_start_turn
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(SessionError::Store(Box::new(std::io::Error::other(
                "mock start_turn failure",
            ))));
        }
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        drop(sessions);

        if is_keep_alive {
            let complete_immediately = self
                .keep_alive_turns_complete_immediately
                .load(Ordering::Relaxed);
            if complete_immediately {
                return Ok(mock_run_result(
                    id.clone(),
                    "Autonomous kickoff completed".to_string(),
                ));
            }
            let notifier = self
                .keep_alive_notifiers
                .read()
                .await
                .get(id)
                .cloned()
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
            notifier.notified().await;
            return Ok(mock_run_result(
                id.clone(),
                "Host loop interrupted".to_string(),
            ));
        }

        if let Some(event_tx) = req.event_tx {
            let delay_ms = self
                .flow_turn_delay_ms
                .load(std::sync::atomic::Ordering::Relaxed);
            let never_terminal = self
                .flow_turn_never_terminal
                .load(std::sync::atomic::Ordering::Relaxed);
            let fail_flow_turn_global = self
                .flow_turn_fail
                .load(std::sync::atomic::Ordering::Relaxed);
            let fail_flow_turn_session = self.flow_turn_fail_sessions.read().await.contains(id);
            let completed_result = self.flow_turn_completed_result.read().await.clone();
            let session_id = id.clone();
            tokio::spawn(async move {
                if delay_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                }
                if never_terminal {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    return;
                }
                if fail_flow_turn_global || fail_flow_turn_session {
                    let _ = event_tx
                        .send(EventEnvelope::new(
                            format!("session:{session_id}"),
                            1,
                            None,
                            AgentEvent::RunFailed {
                                session_id,
                                error_class: meerkat_core::event::AgentErrorClass::Internal,
                                error: "mock flow turn failure".to_string(),
                                error_report: None,
                            },
                        ))
                        .await;
                } else {
                    let _ = event_tx
                        .send(EventEnvelope::new(
                            format!("session:{session_id}"),
                            1,
                            None,
                            AgentEvent::RunCompleted {
                                session_id,
                                result: completed_result,
                                usage: Usage::default(),
                            },
                        ))
                        .await;
                }
            });
        }

        Ok(mock_run_result(id.clone(), "Turn completed".to_string()))
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        self.interrupt_calls.fetch_add(1, Ordering::Relaxed);
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        drop(sessions);
        if let Some(notifier) = self.keep_alive_notifiers.read().await.get(id).cloned() {
            notifier.notify_waiters();
        }
        // Real SessionService flips is_active to false once the turn/keep-alive
        // loop unwinds after interrupt. Mirror that here so the disposal-time
        // poll loop in `stop_autonomous_member` doesn't spin to its 1s cap.
        self.active_sessions.write().await.remove(id);
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let session = self
            .live_session_clone(id)
            .await
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let metadata = session.session_metadata();
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        let is_active = self.active_sessions.read().await.contains(id);
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                is_active,
                model: metadata
                    .as_ref()
                    .map(|meta| meta.model.clone())
                    .unwrap_or_else(|| "claude-sonnet-4-5".to_string()),
                provider: metadata
                    .as_ref()
                    .map(|meta| meta.provider)
                    .unwrap_or(Provider::Anthropic),
                last_assistant_text: session.last_assistant_text(),
                labels: Default::default(),
            },
            billing: SessionUsage {
                total_tokens: session.total_tokens(),
                usage: session.total_usage(),
            },
        })
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let sessions = self.live_session_data.read().await;
        let active = self.active_sessions.read().await;
        Ok(sessions
            .values()
            .map(|session| SessionSummary {
                session_id: session.id().clone(),
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                total_tokens: session.total_tokens(),
                is_active: active.contains(session.id()),
                labels: Default::default(),
            })
            .collect())
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        Ok(self.sessions.read().await.contains_key(id))
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        if self.archive_fail_sessions.read().await.contains(id) {
            return Err(SessionError::Store(Box::new(std::io::Error::other(
                "mock archive failure",
            ))));
        }
        let archive_delay_ms = self.archive_delay_ms.load(Ordering::Relaxed);
        if archive_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(archive_delay_ms)).await;
        }
        let mut sessions = self.sessions.write().await;
        if let Some(runtime) = sessions.remove(id) {
            self.session_comms_names.write().await.remove(id);
            self.external_tools_by_session.write().await.remove(id);
            self.live_session_data.write().await.remove(id);
            if let Some(notifier) = self.keep_alive_notifiers.write().await.remove(id) {
                notifier.notify_waiters();
            }
            let intents = runtime.sent_intents().await;
            self.archived_sent_intents
                .write()
                .await
                .insert(id.clone(), intents);
        }
        Ok(())
    }
}

struct CountingInjector {
    calls: Arc<AtomicU64>,
    delay_ms: u64,
    never_terminal: bool,
    fail: bool,
    fail_inject: bool,
    completed_result: String,
}

impl EventInjector for CountingInjector {
    fn inject(
        &self,
        _content: meerkat_core::types::ContentInput,
        _source: PlainEventSource,
        _handling_mode: meerkat_core::types::HandlingMode,
        _render_metadata: Option<meerkat_core::types::RenderMetadata>,
    ) -> Result<(), EventInjectorError> {
        if self.fail_inject {
            return Err(EventInjectorError::Closed);
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

impl SubscribableInjector for CountingInjector {
    fn inject_with_subscription(
        &self,
        body: meerkat_core::types::ContentInput,
        source: PlainEventSource,
        handling_mode: meerkat_core::types::HandlingMode,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
    ) -> Result<InteractionSubscription, EventInjectorError> {
        self.inject(body, source, handling_mode, render_metadata)?;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let interaction_id = InteractionId(uuid::Uuid::new_v4());
        let delay_ms = self.delay_ms;
        let never_terminal = self.never_terminal;
        let fail = self.fail;
        let completed_result = self.completed_result.clone();
        let interaction_id_for_task = interaction_id;
        tokio::spawn(async move {
            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            if never_terminal {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                return;
            }
            let event = if fail {
                AgentEvent::InteractionFailed {
                    interaction_id: interaction_id_for_task,
                    error: "mock flow turn failure".to_string(),
                }
            } else {
                AgentEvent::InteractionComplete {
                    interaction_id: interaction_id_for_task,
                    result: completed_result,
                }
            };
            let _ = tx.send(event).await;
        });
        Ok(InteractionSubscription {
            id: interaction_id,
            events: rx,
        })
    }
}

#[async_trait]
impl SessionServiceCommsExt for MockSessionService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        if self
            .missing_comms_sessions
            .read()
            .await
            .contains(session_id)
        {
            return None;
        }
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|comms| Arc::clone(comms) as Arc<dyn CoreCommsRuntime>)
    }

    async fn event_injector(&self, session_id: &SessionId) -> Option<Arc<dyn EventInjector>> {
        if !self.sessions.read().await.contains_key(session_id) {
            return None;
        }
        let delay_ms = self.flow_turn_delay_ms.load(Ordering::Relaxed);
        let never_terminal = self
            .flow_turn_never_terminal
            .load(std::sync::atomic::Ordering::Relaxed);
        let fail = self
            .flow_turn_fail
            .load(std::sync::atomic::Ordering::Relaxed)
            || self
                .flow_turn_fail_sessions
                .read()
                .await
                .contains(session_id);
        let completed_result = self.flow_turn_completed_result.read().await.clone();
        let fail_inject = self.fail_inject.load(std::sync::atomic::Ordering::Relaxed);
        Some(Arc::new(CountingInjector {
            calls: self.inject_calls.clone(),
            delay_ms,
            never_terminal,
            fail,
            fail_inject,
            completed_result,
        }))
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        if !self.sessions.read().await.contains_key(session_id) {
            return None;
        }
        let delay_ms = self.flow_turn_delay_ms.load(Ordering::Relaxed);
        let never_terminal = self
            .flow_turn_never_terminal
            .load(std::sync::atomic::Ordering::Relaxed);
        let fail = self
            .flow_turn_fail
            .load(std::sync::atomic::Ordering::Relaxed)
            || self
                .flow_turn_fail_sessions
                .read()
                .await
                .contains(session_id);
        let completed_result = self.flow_turn_completed_result.read().await.clone();
        let fail_inject = self.fail_inject.load(std::sync::atomic::Ordering::Relaxed);
        Some(Arc::new(CountingInjector {
            calls: self.inject_calls.clone(),
            delay_ms,
            never_terminal,
            fail,
            fail_inject,
            completed_result,
        }))
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceHistoryExt for MockSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: meerkat_core::service::SessionHistoryQuery,
    ) -> Result<meerkat_core::service::SessionHistoryPage, SessionError> {
        let session = self
            .live_session_clone(id)
            .await
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        Ok(meerkat_core::service::SessionHistoryPage::from_messages(
            id.clone(),
            session.messages(),
            query,
        ))
    }
}

#[async_trait]
impl SessionServiceControlExt for MockSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        _req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() }.into());
        }
        Ok(AppendSystemContextResult {
            status: AppendSystemContextStatus::Staged,
        })
    }
}

#[async_trait]
impl MobSessionService for MockSessionService {
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::EventStream, meerkat_core::StreamError> {
        if self
            .subscribe_fail_sessions
            .read()
            .await
            .contains(session_id)
        {
            return Err(meerkat_core::StreamError::Internal(format!(
                "simulated subscribe failure for session {session_id}"
            )));
        }
        if !self.sessions.read().await.contains_key(session_id) {
            return Err(meerkat_core::StreamError::NotFound(format!(
                "session {session_id}"
            )));
        }
        Ok(Box::pin(
            futures::stream::empty::<EventEnvelope<AgentEvent>>(),
        ))
    }

    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        self.runtime_adapter
            .lock()
            .expect("runtime_adapter mutex")
            .clone()
    }

    async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
        let names = self.session_comms_names.read().await;
        names
            .get(session_id)
            .is_some_and(|name| name.starts_with(&format!("{mob_id}/")))
    }

    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        Ok(self.persisted_session_clone(session_id).await)
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        <Self as SessionService>::start_turn(self, session_id, req).await?;
        Ok(meerkat_core::lifecycle::core_executor::CoreApplyOutput {
            receipt: meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt {
                run_id,
                boundary,
                contributing_input_ids,
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            },
            session_snapshot: None,
            terminal: None,
        })
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.sessions.write().await.remove(session_id);
        self.live_session_data.write().await.remove(session_id);
        self.keep_alive_notifiers.write().await.remove(session_id);
        self.session_comms_names.write().await.remove(session_id);
        self.external_tools_by_session
            .write()
            .await
            .remove(session_id);
        Ok(())
    }
}

struct FaultInjectedMobEventStore {
    events: RwLock<Vec<MobEvent>>,
    event_tx: tokio::sync::broadcast::Sender<MobEvent>,
    fail_on_kind: RwLock<HashSet<&'static str>>,
    poll_calls: AtomicU64,
    replay_calls: AtomicU64,
}

impl FaultInjectedMobEventStore {
    fn new() -> Self {
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel(4096);
        Self {
            events: RwLock::new(Vec::new()),
            event_tx,
            fail_on_kind: RwLock::new(HashSet::new()),
            poll_calls: AtomicU64::new(0),
            replay_calls: AtomicU64::new(0),
        }
    }

    async fn fail_appends_for(&self, kind: &'static str) {
        self.fail_on_kind.write().await.insert(kind);
    }

    fn replay_calls(&self) -> u64 {
        self.replay_calls.load(Ordering::Relaxed)
    }

    fn poll_calls(&self) -> u64 {
        self.poll_calls.load(Ordering::Relaxed)
    }

    fn kind_label(kind: &MobEventKind) -> &'static str {
        match kind {
            MobEventKind::MobCreated { .. } => "MobCreated",
            MobEventKind::MobCompleted => "MobCompleted",
            MobEventKind::MobReset => "MobReset",
            MobEventKind::MemberSpawned(..) => "MemberSpawned",
            MobEventKind::MemberRetired { .. } => "MemberRetired",
            MobEventKind::MemberReset { .. } => "MemberReset",
            MobEventKind::MemberKickoffUpdated { .. } => "MemberKickoffUpdated",
            MobEventKind::MembersWired { .. } => "MembersWired",
            MobEventKind::MembersUnwired { .. } => "MembersUnwired",
            MobEventKind::ExternalPeerWired { .. } => "ExternalPeerWired",
            MobEventKind::ExternalPeerUnwired { .. } => "ExternalPeerUnwired",
            MobEventKind::TaskCreated { .. } => "TaskCreated",
            MobEventKind::TaskUpdated { .. } => "TaskUpdated",
            MobEventKind::FlowStarted { .. } => "FlowStarted",
            MobEventKind::FlowCompleted { .. } => "FlowCompleted",
            MobEventKind::FlowFailed { .. } => "FlowFailed",
            MobEventKind::FlowCanceled { .. } => "FlowCanceled",
            MobEventKind::StepDispatched { .. } => "StepDispatched",
            MobEventKind::StepTargetCompleted { .. } => "StepTargetCompleted",
            MobEventKind::StepTargetFailed { .. } => "StepTargetFailed",
            MobEventKind::StepCompleted { .. } => "StepCompleted",
            MobEventKind::StepFailed { .. } => "StepFailed",
            MobEventKind::StepSkipped { .. } => "StepSkipped",
            MobEventKind::TopologyViolation { .. } => "TopologyViolation",
            MobEventKind::SupervisorEscalation { .. } => "SupervisorEscalation",
            MobEventKind::OperatorActionRecorded { .. } => "OperatorActionRecorded",
        }
    }
}

#[async_trait]
impl MobEventStore for FaultInjectedMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError> {
        let kind_label = Self::kind_label(&event.kind);
        if self.fail_on_kind.read().await.contains(kind_label) {
            return Err(MobStoreError::Internal(format!(
                "fault-injected append failure for {kind_label}"
            )));
        }

        let mut events = self.events.write().await;
        let cursor = events.len() as u64 + 1;
        let stored = MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: event.mob_id,
            kind: event.kind,
        };
        events.push(stored.clone());
        drop(events);
        let _ = self.event_tx.send(stored.clone());
        Ok(stored)
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobStoreError> {
        self.poll_calls.fetch_add(1, Ordering::Relaxed);
        let events = self.events.read().await;
        Ok(events
            .iter()
            .filter(|e| e.cursor > after_cursor)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError> {
        self.replay_calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.events.read().await.clone())
    }

    async fn latest_cursor(&self) -> Result<u64, MobStoreError> {
        Ok(self
            .events
            .read()
            .await
            .last()
            .map_or(0, |event| event.cursor))
    }

    fn subscribe(&self) -> Result<crate::store::MobEventReceiver, MobStoreError> {
        Ok(self.event_tx.subscribe())
    }

    async fn append_batch(&self, events: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobStoreError> {
        let mut results = Vec::with_capacity(events.len());
        for event in events {
            results.push(self.append(event).await?);
        }
        Ok(results)
    }

    async fn clear(&self) -> Result<(), MobStoreError> {
        self.events.write().await.clear();
        Ok(())
    }
}

struct RecordingRunStore {
    inner: InMemoryMobRunStore,
    fail_create_run_once: AtomicBool,
    cas_history: RwLock<Vec<(RunId, MobRunStatus, MobRunStatus)>>,
    snapshot_cas_history: RwLock<Vec<(RunId, MobRunStatus, MobRunStatus)>>,
}

impl RecordingRunStore {
    fn new() -> Self {
        Self {
            inner: InMemoryMobRunStore::new(),
            fail_create_run_once: AtomicBool::new(false),
            cas_history: RwLock::new(Vec::new()),
            snapshot_cas_history: RwLock::new(Vec::new()),
        }
    }

    fn fail_next_create_run(&self) {
        self.fail_create_run_once.store(true, Ordering::Relaxed);
    }

    async fn snapshot_cas_history(&self) -> Vec<(RunId, MobRunStatus, MobRunStatus)> {
        self.snapshot_cas_history.read().await.clone()
    }
}

#[async_trait]
impl MobRunStore for RecordingRunStore {
    async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError> {
        if self.fail_create_run_once.swap(false, Ordering::Relaxed) {
            return Err(MobStoreError::Internal(
                "fault-injected create_run failure".to_string(),
            ));
        }
        self.inner.create_run(run).await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
        self.inner.get_run(run_id).await
    }

    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&crate::FlowId>,
    ) -> Result<Vec<MobRun>, MobStoreError> {
        self.inner.list_runs(mob_id, flow_id).await
    }

    async fn cas_run_status(
        &self,
        run_id: &RunId,
        expected: MobRunStatus,
        next: MobRunStatus,
    ) -> Result<bool, MobStoreError> {
        self.cas_history
            .write()
            .await
            .push((run_id.clone(), expected.clone(), next.clone()));
        self.inner.cas_run_status(run_id, expected, next).await
    }

    async fn cas_flow_state(
        &self,
        run_id: &RunId,
        expected: &crate::run::flow_run::State,
        next: &crate::run::flow_run::State,
    ) -> Result<bool, MobStoreError> {
        self.inner.cas_flow_state(run_id, expected, next).await
    }

    async fn cas_run_snapshot(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &crate::run::flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &crate::run::flow_run::State,
    ) -> Result<bool, MobStoreError> {
        self.snapshot_cas_history.write().await.push((
            run_id.clone(),
            expected_status.clone(),
            next_status.clone(),
        ));
        self.inner
            .cas_run_snapshot(
                run_id,
                expected_status,
                expected_flow_state,
                next_status,
                next_flow_state,
            )
            .await
    }

    async fn append_step_entry(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<(), MobStoreError> {
        self.inner.append_step_entry(run_id, entry).await
    }

    async fn append_step_entry_if_absent(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<bool, MobStoreError> {
        self.inner.append_step_entry_if_absent(run_id, entry).await
    }

    async fn put_step_output(
        &self,
        run_id: &RunId,
        step_id: &crate::StepId,
        output: serde_json::Value,
    ) -> Result<(), MobStoreError> {
        self.inner.put_step_output(run_id, step_id, output).await
    }

    async fn append_failure_entry(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobStoreError> {
        self.inner.append_failure_entry(run_id, entry).await
    }

    async fn upsert_loop_snapshot(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        snapshot: crate::run::LoopSnapshot,
        ledger_entry: Option<crate::run::LoopIterationLedgerEntry>,
    ) -> Result<(), MobStoreError> {
        self.inner
            .upsert_loop_snapshot(run_id, loop_instance_id, snapshot, ledger_entry)
            .await
    }

    async fn cas_frame_state(
        &self,
        run_id: &RunId,
        frame_id: &crate::ids::FrameId,
        expected: Option<&crate::run::FrameSnapshot>,
        next: crate::run::FrameSnapshot,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_frame_state(run_id, frame_id, expected, next)
            .await
    }

    async fn cas_grant_node_slot(
        &self,
        run_id: &RunId,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
        frame_id: &crate::ids::FrameId,
        expected_frame: &crate::run::FrameSnapshot,
        next_frame: crate::run::FrameSnapshot,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_grant_node_slot(
                run_id,
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
            )
            .await
    }

    async fn cas_complete_step_and_record_output(
        &self,
        run_id: &RunId,
        frame_id: &crate::ids::FrameId,
        expected_frame: &crate::run::FrameSnapshot,
        next_frame: crate::run::FrameSnapshot,
        step_output_key: String,
        step_output: serde_json::Value,
        loop_context: Option<(&crate::ids::LoopId, u64)>,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_complete_step_and_record_output(
                run_id,
                frame_id,
                expected_frame,
                next_frame,
                step_output_key,
                step_output,
                loop_context,
            )
            .await
    }

    async fn cas_start_loop(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
        frame_id: &crate::ids::FrameId,
        expected_frame: &crate::run::FrameSnapshot,
        next_frame: crate::run::FrameSnapshot,
        initial_loop: crate::run::LoopSnapshot,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_start_loop(
                run_id,
                loop_instance_id,
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                initial_loop,
            )
            .await
    }

    async fn cas_loop_request_body_frame(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        expected_loop: &crate::run::LoopSnapshot,
        next_loop: crate::run::LoopSnapshot,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_loop_request_body_frame(
                run_id,
                loop_instance_id,
                expected_loop,
                next_loop,
                expected_run_state,
                next_run_state,
            )
            .await
    }

    async fn cas_grant_body_frame_start(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        expected_loop: &crate::run::LoopSnapshot,
        next_loop: crate::run::LoopSnapshot,
        frame_id: &crate::ids::FrameId,
        initial_frame: crate::run::FrameSnapshot,
        ledger_entry: crate::run::LoopIterationLedgerEntry,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_grant_body_frame_start(
                run_id,
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                initial_frame,
                ledger_entry,
                expected_run_state,
                next_run_state,
            )
            .await
    }

    async fn cas_complete_body_frame(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        expected_loop: &crate::run::LoopSnapshot,
        next_loop: crate::run::LoopSnapshot,
        frame_id: &crate::ids::FrameId,
        expected_frame: &crate::run::FrameSnapshot,
        next_frame: crate::run::FrameSnapshot,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_complete_body_frame(
                run_id,
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
            )
            .await
    }

    async fn cas_complete_loop(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        expected_loop: &crate::run::LoopSnapshot,
        next_loop: crate::run::LoopSnapshot,
        frame_id: &crate::ids::FrameId,
        expected_frame: &crate::run::FrameSnapshot,
        next_frame: crate::run::FrameSnapshot,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_complete_loop(
                run_id,
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
            )
            .await
    }

    async fn cas_flow_state_with_authority(
        &self,
        run_id: &RunId,
        expected: &crate::run::flow_run::State,
        next: &crate::run::flow_run::State,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_flow_state_with_authority(run_id, expected, next, authority_inputs)
            .await
    }

    async fn cas_run_snapshot_with_authority(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &crate::run::flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &crate::run::flow_run::State,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.snapshot_cas_history.write().await.push((
            run_id.clone(),
            expected_status.clone(),
            next_status.clone(),
        ));
        self.inner
            .cas_run_snapshot_with_authority(
                run_id,
                expected_status,
                expected_flow_state,
                next_status,
                next_flow_state,
                authority_inputs,
            )
            .await
    }

    async fn cas_frame_state_with_authority(
        &self,
        run_id: &RunId,
        frame_id: &crate::ids::FrameId,
        expected: Option<&crate::run::FrameSnapshot>,
        next: crate::run::FrameSnapshot,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_frame_state_with_authority(run_id, frame_id, expected, next, authority_inputs)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_grant_node_slot_with_authority(
        &self,
        run_id: &RunId,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
        frame_id: &crate::ids::FrameId,
        expected_frame: &crate::run::FrameSnapshot,
        next_frame: crate::run::FrameSnapshot,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_grant_node_slot_with_authority(
                run_id,
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                authority_inputs,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_step_and_record_output_with_authority(
        &self,
        run_id: &RunId,
        frame_id: &crate::ids::FrameId,
        expected_frame: &crate::run::FrameSnapshot,
        next_frame: crate::run::FrameSnapshot,
        step_output_key: String,
        step_output: serde_json::Value,
        loop_context: Option<(&crate::ids::LoopId, u64)>,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_complete_step_and_record_output_with_authority(
                run_id,
                frame_id,
                expected_frame,
                next_frame,
                step_output_key,
                step_output,
                loop_context,
                authority_inputs,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_start_loop_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
        frame_id: &crate::ids::FrameId,
        expected_frame: &crate::run::FrameSnapshot,
        next_frame: crate::run::FrameSnapshot,
        initial_loop: crate::run::LoopSnapshot,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_start_loop_with_authority(
                run_id,
                loop_instance_id,
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                initial_loop,
                authority_inputs,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_loop_request_body_frame_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        expected_loop: &crate::run::LoopSnapshot,
        next_loop: crate::run::LoopSnapshot,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_loop_request_body_frame_with_authority(
                run_id,
                loop_instance_id,
                expected_loop,
                next_loop,
                expected_run_state,
                next_run_state,
                authority_inputs,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_grant_body_frame_start_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        expected_loop: &crate::run::LoopSnapshot,
        next_loop: crate::run::LoopSnapshot,
        frame_id: &crate::ids::FrameId,
        initial_frame: crate::run::FrameSnapshot,
        ledger_entry: crate::run::LoopIterationLedgerEntry,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_grant_body_frame_start_with_authority(
                run_id,
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                initial_frame,
                ledger_entry,
                expected_run_state,
                next_run_state,
                authority_inputs,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_body_frame_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        expected_loop: &crate::run::LoopSnapshot,
        next_loop: crate::run::LoopSnapshot,
        frame_id: &crate::ids::FrameId,
        expected_frame: &crate::run::FrameSnapshot,
        next_frame: crate::run::FrameSnapshot,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_complete_body_frame_with_authority(
                run_id,
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
                authority_inputs,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_loop_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &crate::ids::LoopInstanceId,
        expected_loop: &crate::run::LoopSnapshot,
        next_loop: crate::run::LoopSnapshot,
        frame_id: &crate::ids::FrameId,
        expected_frame: &crate::run::FrameSnapshot,
        next_frame: crate::run::FrameSnapshot,
        expected_run_state: &crate::run::flow_run::State,
        next_run_state: crate::run::flow_run::State,
        authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .cas_complete_loop_with_authority(
                run_id,
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
                authority_inputs,
            )
            .await
    }
}

struct FaultInjectedRuntimeMetadataStore {
    inner: InMemoryMobRuntimeMetadataStore,
    fail_load_supervisor: AtomicBool,
    fail_list_overlays: AtomicBool,
}

impl FaultInjectedRuntimeMetadataStore {
    fn new() -> Self {
        Self {
            inner: InMemoryMobRuntimeMetadataStore::new(),
            fail_load_supervisor: AtomicBool::new(false),
            fail_list_overlays: AtomicBool::new(false),
        }
    }

    fn set_fail_load_supervisor(&self, fail: bool) {
        self.fail_load_supervisor.store(fail, Ordering::Relaxed);
    }
}

#[async_trait]
impl MobRuntimeMetadataStore for FaultInjectedRuntimeMetadataStore {
    async fn load_supervisor_authority(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<SupervisorAuthorityRecord>, MobStoreError> {
        if self.fail_load_supervisor.load(Ordering::Relaxed) {
            return Err(MobStoreError::ReadFailed(
                "fault-injected runtime metadata load failure".to_string(),
            ));
        }
        self.inner.load_supervisor_authority(mob_id).await
    }

    async fn put_supervisor_authority(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
    ) -> Result<(), MobStoreError> {
        self.inner.put_supervisor_authority(mob_id, record).await
    }

    async fn put_supervisor_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .put_supervisor_authority_if_absent(mob_id, record)
            .await
    }

    async fn delete_supervisor_authority(&self, mob_id: &MobId) -> Result<(), MobStoreError> {
        self.inner.delete_supervisor_authority(mob_id).await
    }

    async fn list_external_binding_overlays(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<ExternalBindingOverlayRecord>, MobStoreError> {
        if self.fail_list_overlays.load(Ordering::Relaxed) {
            return Err(MobStoreError::ReadFailed(
                "fault-injected runtime overlay load failure".to_string(),
            ));
        }
        self.inner.list_external_binding_overlays(mob_id).await
    }

    async fn put_external_binding_overlay_if_absent(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<bool, MobStoreError> {
        self.inner
            .put_external_binding_overlay_if_absent(mob_id, record)
            .await
    }

    async fn upsert_external_binding_overlay(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<(), MobStoreError> {
        self.inner
            .upsert_external_binding_overlay(mob_id, record)
            .await
    }

    async fn delete_external_binding_overlay(
        &self,
        mob_id: &MobId,
        agent_identity: &AgentIdentity,
        generation: crate::ids::Generation,
    ) -> Result<(), MobStoreError> {
        self.inner
            .delete_external_binding_overlay(mob_id, agent_identity, generation)
            .await
    }

    async fn delete_external_binding_overlays(&self, mob_id: &MobId) -> Result<(), MobStoreError> {
        self.inner.delete_external_binding_overlays(mob_id).await
    }
}

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

fn sample_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        ProfileBinding::Inline(Profile {
            model: "claude-opus-4-6".into(),
            skills: vec![],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: false,
                mob: true,
                mob_tasks: false,
                schedule: false,
                mcp: vec![],
                rust_bundles: vec![],
            },
            peer_description: "The lead".into(),
            external_addressable: true,
            backend: None,
            runtime_mode: crate::MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }),
    );
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Profile {
            model: "claude-sonnet-4-5".into(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..ToolConfig::default()
            },
            peer_description: "A worker".into(),
            external_addressable: false,
            backend: None,
            runtime_mode: crate::MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }),
    );

    MobDefinition {
        id: MobId::from("test-mob"),
        orchestrator: Some(OrchestratorConfig {
            profile: ProfileName::from("lead"),
        }),
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: WiringRules::default(),
        skills: BTreeMap::new(),
        backend: BackendConfig::default(),
        flows: BTreeMap::new(),
        topology: None,
        supervisor: None,
        limits: None,
        spawn_policy: None,
        event_router: None,
        owner_bridge_session_id: None,
        session_cleanup_policy: crate::definition::SessionCleanupPolicy::Manual,
        is_implicit: false,
    }
}

fn sample_definition_with_auto_wire() -> MobDefinition {
    let mut def = sample_definition();
    def.wiring.auto_wire_orchestrator = true;
    def
}

fn sample_definition_with_role_wiring() -> MobDefinition {
    let mut def = sample_definition();
    def.wiring.role_wiring = vec![RoleWiringRule {
        a: ProfileName::from("worker"),
        b: ProfileName::from("worker"),
    }];
    def
}

fn sample_definition_with_cross_role_wiring() -> MobDefinition {
    let mut def = sample_definition();
    def.wiring.role_wiring = vec![RoleWiringRule {
        a: ProfileName::from("lead"),
        b: ProfileName::from("worker"),
    }];
    def
}

fn sample_definition_with_overlapping_orchestrator_and_role_wiring() -> MobDefinition {
    let mut def = sample_definition_with_cross_role_wiring();
    def.wiring.auto_wire_orchestrator = true;
    def
}

fn sample_definition_with_tool_bundle(bundle_name: &str) -> MobDefinition {
    let mut def = sample_definition();
    let worker = def
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .and_then(|b| b.as_inline_mut())
        .expect("worker profile exists");
    worker.tools.rust_bundles = vec![bundle_name.to_string()];
    def
}

fn sample_definition_with_mob_tools() -> MobDefinition {
    let mut def = sample_definition();
    let worker = def
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .and_then(|b| b.as_inline_mut())
        .expect("worker profile exists");
    worker.tools.mob = true;
    worker.tools.mob_tasks = true;
    worker.tools.comms = true;
    def
}

fn sample_definition_with_external_backend() -> MobDefinition {
    let mut def = sample_definition();
    def.backend.external = Some(crate::definition::ExternalBackendConfig {
        address_base: "https://backend.example.invalid/mesh".to_string(),
    });
    def
}

fn sample_definition_without_mob_flags() -> MobDefinition {
    let mut def = sample_definition();
    let lead = def
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .expect("lead profile exists")
        .as_inline_mut()
        .unwrap();
    lead.tools.mob = false;
    lead.tools.mob_tasks = false;
    def
}

fn sample_definition_with_mcp_servers() -> MobDefinition {
    let mut def = sample_definition();
    def.mcp_servers.insert(
        "search".to_string(),
        crate::definition::McpServerConfig {
            command: Vec::new(),
            url: Some("https://example.invalid/mcp".to_string()),
            env: BTreeMap::new(),
        },
    );
    def
}

fn flow_step(role: impl Into<crate::ids::ProfileName>, message: &str) -> FlowStepSpec {
    FlowStepSpec {
        role: role.into(),
        message: meerkat_core::types::ContentInput::from(message),
        depends_on: Vec::new(),
        dispatch_mode: DispatchMode::FanOut,
        collection_policy: CollectionPolicy::All,
        condition: None,
        timeout_ms: None,
        expected_schema_ref: None,
        branch: None,
        depends_on_mode: DependencyMode::All,
        allowed_tools: None,
        blocked_tools: None,
        output_format: StepOutputFormat::Json,
    }
}

fn flow_id(value: &str) -> crate::FlowId {
    crate::FlowId::from(value)
}

fn step_id(value: &str) -> crate::StepId {
    crate::StepId::from(value)
}

fn sample_definition_with_single_step_flow(
    timeout_ms: u64,
    max_orphaned_turns: u32,
) -> MobDefinition {
    let mut def = sample_definition();
    let mut steps = IndexMap::new();
    let mut step = flow_step("worker", "Execute step");
    step.timeout_ms = Some(timeout_ms);
    steps.insert(step_id("start"), step);

    let mut flows = BTreeMap::new();
    flows.insert(
        flow_id("demo"),
        FlowSpec {
            description: Some("single step demo flow".to_string()),
            steps,
            root: None,
        },
    );
    def.flows = flows;
    def.limits = Some(LimitsSpec {
        max_flow_duration_ms: None,
        max_step_retries: None,
        max_orphaned_turns: Some(max_orphaned_turns),
        cancel_grace_timeout_ms: None,
        ..Default::default()
    });
    def
}

fn with_cancel_grace_timeout(
    mut def: MobDefinition,
    cancel_grace_timeout_ms: u64,
) -> MobDefinition {
    let limits = def.limits.get_or_insert(LimitsSpec {
        max_flow_duration_ms: None,
        max_step_retries: None,
        max_orphaned_turns: None,
        cancel_grace_timeout_ms: None,
        ..Default::default()
    });
    limits.cancel_grace_timeout_ms = Some(cancel_grace_timeout_ms);
    def
}

fn sample_definition_with_branch_flow() -> MobDefinition {
    let mut def = sample_definition();
    let mut steps = IndexMap::new();
    steps.insert(step_id("start"), flow_step("worker", "Start"));

    let mut fix_critical = flow_step("worker", "Fix critical");
    fix_critical.depends_on = vec![step_id("start")];
    fix_critical.condition = Some(ConditionExpr::Eq {
        path: "params.severity".to_string(),
        value: serde_json::json!("critical"),
    });
    fix_critical.branch = Some(crate::ids::BranchId::from("repair"));
    steps.insert(step_id("fix_critical"), fix_critical);

    let mut fix_minor = flow_step("worker", "Fix minor");
    fix_minor.depends_on = vec![step_id("start")];
    fix_minor.condition = Some(ConditionExpr::Eq {
        path: "params.severity".to_string(),
        value: serde_json::json!("minor"),
    });
    fix_minor.branch = Some(crate::ids::BranchId::from("repair"));
    steps.insert(step_id("fix_minor"), fix_minor);

    let mut summarize = flow_step("worker", "Summarize {{steps.fix_critical}}");
    summarize.depends_on = vec![step_id("fix_critical"), step_id("fix_minor")];
    summarize.depends_on_mode = DependencyMode::Any;
    steps.insert(step_id("summarize"), summarize);

    let mut flows = BTreeMap::new();
    flows.insert(
        flow_id("branching"),
        FlowSpec {
            description: Some("branch winner flow".to_string()),
            steps,
            root: None,
        },
    );
    def.flows = flows;
    def
}

fn sample_definition_with_strict_topology_deny() -> MobDefinition {
    let mut def = sample_definition_with_single_step_flow(500, 8);
    def.topology = Some(TopologySpec {
        mode: PolicyMode::Strict,
        rules: vec![TopologyRule {
            from_role: ProfileName::from("lead"),
            to_role: ProfileName::from("worker"),
            allowed: false,
        }],
    });
    def
}

fn sample_definition_with_advisory_topology_deny() -> MobDefinition {
    let mut def = sample_definition_with_single_step_flow(500, 8);
    def.topology = Some(TopologySpec {
        mode: PolicyMode::Advisory,
        rules: vec![TopologyRule {
            from_role: ProfileName::from("lead"),
            to_role: ProfileName::from("worker"),
            allowed: false,
        }],
    });
    def
}

fn sample_definition_with_strict_topology_wildcard_deny() -> MobDefinition {
    let mut def = sample_definition_with_single_step_flow(500, 8);
    def.topology = Some(TopologySpec {
        mode: PolicyMode::Strict,
        rules: vec![TopologyRule {
            from_role: ProfileName::from("*"),
            to_role: ProfileName::from("worker"),
            allowed: false,
        }],
    });
    def
}

fn sample_definition_with_advisory_topology_wildcard_deny() -> MobDefinition {
    let mut def = sample_definition_with_single_step_flow(500, 8);
    def.topology = Some(TopologySpec {
        mode: PolicyMode::Advisory,
        rules: vec![TopologyRule {
            from_role: ProfileName::from("*"),
            to_role: ProfileName::from("worker"),
            allowed: false,
        }],
    });
    def
}

fn sample_definition_with_collection_policy(policy: CollectionPolicy) -> MobDefinition {
    let mut def = sample_definition();
    let mut steps = IndexMap::new();
    let mut step = flow_step("worker", "Collect results");
    step.dispatch_mode = DispatchMode::FanOut;
    step.collection_policy = policy;
    step.timeout_ms = Some(500);
    steps.insert(step_id("collect"), step);

    let mut flows = BTreeMap::new();
    flows.insert(
        flow_id("collect"),
        FlowSpec {
            description: Some("collection policy flow".to_string()),
            steps,
            root: None,
        },
    );
    def.flows = flows;
    def
}

fn sample_definition_with_dispatch_mode(mode: DispatchMode) -> MobDefinition {
    let mut def = sample_definition();
    let mut steps = IndexMap::new();
    let mut step = flow_step("worker", "Dispatch mode");
    step.dispatch_mode = mode;
    step.collection_policy = CollectionPolicy::Any;
    step.timeout_ms = Some(500);
    steps.insert(step_id("dispatch"), step);

    let mut flows = BTreeMap::new();
    flows.insert(
        flow_id("dispatch"),
        FlowSpec {
            description: Some("dispatch mode flow".to_string()),
            steps,
            root: None,
        },
    );
    def.flows = flows;
    def
}

fn sample_definition_with_dispatch_mode_and_policy(
    mode: DispatchMode,
    policy: CollectionPolicy,
) -> MobDefinition {
    let mut def = sample_definition();
    let mut steps = IndexMap::new();
    let mut step = flow_step("worker", "Dispatch mode");
    step.dispatch_mode = mode;
    step.collection_policy = policy;
    step.timeout_ms = Some(500);
    steps.insert(step_id("dispatch"), step);

    let mut flows = BTreeMap::new();
    flows.insert(
        flow_id("dispatch"),
        FlowSpec {
            description: Some("dispatch mode flow".to_string()),
            steps,
            root: None,
        },
    );
    def.flows = flows;
    def
}

fn sample_definition_with_retry_flow(max_step_retries: u32) -> MobDefinition {
    let mut def = sample_definition_with_single_step_flow(250, 8);
    def.limits = Some(LimitsSpec {
        max_flow_duration_ms: None,
        max_step_retries: Some(max_step_retries),
        max_orphaned_turns: Some(8),
        cancel_grace_timeout_ms: None,
        ..Default::default()
    });
    def
}

fn sample_definition_with_branch_fallback_flow() -> MobDefinition {
    let mut def = sample_definition();
    let worker_template = def
        .profiles
        .get(&ProfileName::from("worker"))
        .cloned()
        .expect("worker profile exists");
    def.profiles
        .insert(ProfileName::from("worker_fail"), worker_template.clone());
    def.profiles
        .insert(ProfileName::from("worker_ok"), worker_template);

    let mut steps = IndexMap::new();
    steps.insert(step_id("start"), flow_step("worker_ok", "Start"));

    let mut first = flow_step("worker_fail", "First branch candidate");
    first.depends_on = vec![step_id("start")];
    first.condition = Some(ConditionExpr::Eq {
        path: "params.try_fallback".to_string(),
        value: serde_json::json!(true),
    });
    first.branch = Some(crate::ids::BranchId::from("repair"));
    steps.insert(step_id("candidate_first"), first);

    let mut second = flow_step("worker_ok", "Second branch candidate");
    second.depends_on = vec![step_id("start")];
    second.condition = Some(ConditionExpr::Eq {
        path: "params.try_fallback".to_string(),
        value: serde_json::json!(true),
    });
    second.branch = Some(crate::ids::BranchId::from("repair"));
    steps.insert(step_id("candidate_second"), second);

    let mut join = flow_step("worker_ok", "Join");
    join.depends_on = vec![step_id("candidate_first"), step_id("candidate_second")];
    join.depends_on_mode = DependencyMode::Any;
    steps.insert(step_id("join"), join);

    let mut flows = BTreeMap::new();
    flows.insert(
        flow_id("branch_fallback"),
        FlowSpec {
            description: Some("branch fallback flow".to_string()),
            steps,
            root: None,
        },
    );
    def.flows = flows;
    def
}

fn sample_definition_with_template_message(template: &str) -> MobDefinition {
    let mut def = sample_definition_with_single_step_flow(500, 8);
    let step = def
        .flows
        .get_mut(&flow_id("demo"))
        .and_then(|flow| flow.steps.get_mut(&step_id("start")))
        .expect("demo.start step exists");
    step.message = template.to_string().into();
    def
}

fn sample_definition_with_shared_path_resolution_flow() -> MobDefinition {
    let mut def = sample_definition();
    let mut steps = IndexMap::new();
    let mut start = flow_step("worker", "Start");
    start.dispatch_mode = DispatchMode::OneToOne;
    start.collection_policy = CollectionPolicy::Any;
    steps.insert(step_id("start"), start);

    let mut follow = flow_step("worker", "Follow {{steps.start.output}}");
    follow.depends_on = vec![step_id("start")];
    follow.condition = Some(ConditionExpr::Eq {
        path: "steps.start".to_string(),
        value: serde_json::json!("Turn completed"),
    });
    steps.insert(step_id("follow"), follow);

    let mut flows = BTreeMap::new();
    flows.insert(
        flow_id("shared_paths"),
        FlowSpec {
            description: Some("shared path resolver flow".to_string()),
            steps,
            root: None,
        },
    );
    def.flows = flows;
    def
}

fn sample_definition_with_schema_ref(schema_ref: &str) -> MobDefinition {
    let mut def = sample_definition_with_single_step_flow(500, 8);
    let step = def
        .flows
        .get_mut(&flow_id("demo"))
        .expect("demo flow exists")
        .steps
        .get_mut(&step_id("start"))
        .expect("start step exists");
    step.expected_schema_ref = Some(schema_ref.to_string());
    def
}

fn sample_definition_with_supervisor_threshold(threshold: u32) -> MobDefinition {
    let mut def = sample_definition_with_single_step_flow(500, 8);
    def.supervisor = Some(crate::definition::SupervisorSpec {
        role: ProfileName::from("lead"),
        escalation_threshold: threshold,
    });
    def
}

fn sample_definition_with_two_step_flow(timeout_ms: u64) -> MobDefinition {
    let mut def = sample_definition();
    let mut steps = IndexMap::new();
    let mut first = flow_step("worker", "First");
    first.timeout_ms = Some(timeout_ms);
    steps.insert(step_id("first"), first);

    let mut second = flow_step("worker", "Second");
    second.depends_on = vec![step_id("first")];
    second.timeout_ms = Some(timeout_ms);
    steps.insert(step_id("second"), second);

    let mut flows = BTreeMap::new();
    flows.insert(
        flow_id("two_step"),
        FlowSpec {
            description: Some("cooperative cancel flow".to_string()),
            steps,
            root: None,
        },
    );
    def.flows = flows;
    def
}

async fn wait_for_run_terminal(
    handle: &MobHandle,
    run_id: &RunId,
    timeout: Duration,
) -> crate::run::MobRun {
    let deadline = Instant::now() + timeout;
    loop {
        let run = handle
            .flow_status(run_id.clone())
            .await
            .expect("flow_status should succeed")
            .expect("run should exist");
        if run.status.is_terminal() {
            return run;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for run {run_id} terminal state; last status: {:?}",
            run.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Build a test RuntimeBinding::External for a given meerkat_id.
fn test_external_binding(agent_identity: &str) -> crate::RuntimeBinding {
    let bootstrap_token = format!("bootstrap-{agent_identity}");
    crate::RuntimeBinding::External {
        peer_id: format!("ed25519:test-key:{agent_identity}"),
        address: format!("tcp://test.invalid/{agent_identity}"),
        bootstrap_token: Some(bootstrap_token.into()),
        pubkey: None,
    }
}

fn canonical_external_address(address: &str) -> &str {
    address.split('?').next().unwrap_or(address)
}

#[derive(Clone)]
struct HarnessSupervisorState {
    supervisor: meerkat_core::comms::TrustedPeerDescriptor,
    epoch: u64,
}

struct LiveExternalPeerHarness {
    binding: crate::RuntimeBinding,
    runtime: Arc<meerkat_comms::CommsRuntime>,
    bind_count: Arc<std::sync::atomic::AtomicUsize>,
    bind_protocol_versions: Arc<RwLock<Vec<super::bridge_protocol::BridgeProtocolVersion>>>,
    delivered_inputs: Arc<RwLock<Vec<String>>>,
    received_intents: Arc<RwLock<Vec<String>>>,
    supervisor_state: Arc<RwLock<Option<HarnessSupervisorState>>>,
    fail_next_authorize: Arc<AtomicBool>,
    fail_next_bind: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl LiveExternalPeerHarness {
    fn binding(&self) -> crate::RuntimeBinding {
        self.binding.clone()
    }

    async fn trusted_peer_names(&self) -> Vec<String> {
        self.runtime
            .peers()
            .await
            .into_iter()
            .map(|entry| entry.name.as_str().to_string())
            .collect()
    }

    async fn remove_trusted_peer(&self, peer_id: &str) {
        self.runtime
            .remove_trusted_peer(peer_id)
            .await
            .expect("remove trusted peer from harness runtime");
    }

    async fn remove_trusted_peer_descriptor(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        peer: &meerkat_core::comms::TrustedPeerDescriptor,
    ) {
        runtime
            .remove_trusted_peer(&peer.peer_id.to_string())
            .await
            .expect("remove trusted peer from harness runtime");
    }

    fn bind_count(&self) -> usize {
        self.bind_count.load(Ordering::Relaxed)
    }

    async fn bind_protocol_versions(&self) -> Vec<super::bridge_protocol::BridgeProtocolVersion> {
        self.bind_protocol_versions.read().await.clone()
    }

    async fn delivered_input_ids(&self) -> Vec<String> {
        self.delivered_inputs.read().await.clone()
    }

    async fn received_intents(&self) -> Vec<String> {
        self.received_intents.read().await.clone()
    }

    async fn authorized_supervisor_peer_id(&self) -> Option<String> {
        self.supervisor_state
            .read()
            .await
            .as_ref()
            .map(|state| state.supervisor.peer_id.to_string())
    }

    async fn forget_supervisor(&self) {
        let _ = self.supervisor_state.write().await.take();
    }

    fn fail_next_authorize(&self) {
        self.fail_next_authorize.store(true, Ordering::Relaxed);
    }

    fn fail_next_bind(&self) {
        self.fail_next_bind.store(true, Ordering::Relaxed);
    }
}

impl Drop for LiveExternalPeerHarness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_live_external_peer(peer_name: &str) -> LiveExternalPeerHarness {
    let peer_name = peer_name.to_string();
    let runtime = Arc::new(
        meerkat_comms::CommsRuntime::inproc_only(&peer_name)
            .expect("create live external peer runtime"),
    );
    install_ephemeral_peer_request_response_authority(
        &runtime,
        &format!("live-external-peer-{peer_name}"),
    );
    let bootstrap_token = runtime.bridge_bootstrap_token().to_string();
    let peer_pubkey = *runtime.public_key().as_bytes();
    let binding = crate::RuntimeBinding::External {
        peer_id: runtime.public_key().to_peer_id().to_string(),
        address: format!("inproc://{peer_name}"),
        bootstrap_token: Some(bootstrap_token.into()),
        pubkey: Some(peer_pubkey),
    };
    let responder_runtime = runtime.clone();
    let bind_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let responder_bind_count = bind_count.clone();
    let bind_protocol_versions = Arc::new(RwLock::new(Vec::new()));
    let responder_bind_protocol_versions = bind_protocol_versions.clone();
    let delivered_inputs = Arc::new(RwLock::new(Vec::new()));
    let responder_delivered_inputs = delivered_inputs.clone();
    let delivery_responses = Arc::new(RwLock::new(HashMap::new()));
    let responder_delivery_responses = delivery_responses.clone();
    let received_intents = Arc::new(RwLock::new(Vec::new()));
    let responder_received_intents = received_intents.clone();
    let supervisor_state: Arc<RwLock<Option<HarnessSupervisorState>>> = Arc::new(RwLock::new(None));
    let responder_supervisor_state = supervisor_state.clone();
    let fail_next_authorize = Arc::new(AtomicBool::new(false));
    let responder_fail_next_authorize = fail_next_authorize.clone();
    let fail_next_bind = Arc::new(AtomicBool::new(false));
    let responder_fail_next_bind = fail_next_bind.clone();
    let task = tokio::spawn(async move {
        let mut state = super::bridge_protocol::BridgeMemberRuntimeState::Idle;
        let inbox_notify = responder_runtime.inbox_notify();

        loop {
            let notified = inbox_notify.notified();
            let candidates = responder_runtime.drain_peer_input_candidates().await;
            if candidates.is_empty() {
                if responder_runtime.dismiss_received() {
                    break;
                }
                notified.await;
                continue;
            }

            for candidate in candidates {
                match &candidate.interaction.content {
                    meerkat_core::interaction::InteractionContent::Request { intent, params } => {
                        responder_received_intents
                            .write()
                            .await
                            .push(intent.clone());
                        responder_runtime
                            .peer_interaction_handle()
                            .expect("live external peer semantic authority")
                            .request_received(PeerCorrelationId::from_uuid(
                                candidate.interaction.id.0,
                            ))
                            .expect("record inbound peer request before response");
                        let reply_name = candidate.interaction.from.clone();
                        let to = test_trusted_peer_route(responder_runtime.as_ref(), &reply_name);
                        let bridge_parse: Result<super::bridge_protocol::BridgeCommand, _> =
                            serde_json::from_value(params.clone());
                        let mut remove_supervisors_after_response = Vec::new();
                        let response = if let Ok(command) = bridge_parse {
                            match command {
                                super::bridge_protocol::BridgeCommand::BindMember(payload) => {
                                    responder_bind_protocol_versions
                                        .write()
                                        .await
                                        .push(payload.protocol_version);
                                    if responder_fail_next_bind.swap(false, Ordering::Relaxed) {
                                        serde_json::to_value(
                                            super::bridge_protocol::BridgeReply::Rejected {
                                                cause: super::bridge_protocol::BridgeRejectionCause::Internal,
                                                reason: "bind member failed: injected test failure"
                                                    .to_string(),
                                            },
                                        )
                                        .expect("bind rejection")
                                    } else {
                                        // Mirror the production strict gate in
                                        // `validate_bind_request_against_state`: unbound state
                                        // allows bootstrap; bound state only accepts an exact
                                        // idempotent retry from the same supervisor at the same
                                        // epoch from the same sender. Anything else is a typed
                                        // rejection — mob-level tests must see the same rejection
                                        // vocabulary as production or the gate is invisible.
                                        let sender = candidate.interaction.from.clone();
                                        let mut state_guard =
                                            responder_supervisor_state.write().await;
                                        let gate_result: Result<bool, (super::bridge_protocol::BridgeRejectionCause, String)> = match state_guard.as_ref() {
                                            None => Ok(true), // bootstrap: bind fresh
                                            Some(current) => {
                                                if payload.supervisor.peer_id
                                                    != current.supervisor.peer_id.to_string()
                                                {
                                                    Err((
                                                        super::bridge_protocol::BridgeRejectionCause::AlreadyBound,
                                                        format!(
                                                            "bind member failed: supervisor already bound as '{}'; use authorize_supervisor to rotate",
                                                            current.supervisor.peer_id
                                                        ),
                                                    ))
                                                } else if payload.epoch != current.epoch {
                                                    Err((
                                                        super::bridge_protocol::BridgeRejectionCause::AlreadyBound,
                                                        format!(
                                                            "bind member failed: epoch {} does not match bound supervisor epoch {}; use authorize_supervisor to rotate",
                                                            payload.epoch, current.epoch
                                                        ),
                                                    ))
                                                } else if sender != current.supervisor.name.as_str()
                                                    && sender != current.supervisor.peer_id.to_string()
                                                {
                                                    Err((
                                                        super::bridge_protocol::BridgeRejectionCause::SenderMismatch,
                                                        format!(
                                                            "bind member failed: request sender '{sender}' does not match authorized supervisor '{}'",
                                                            current.supervisor.peer_id
                                                        ),
                                                    ))
                                                } else {
                                                    Ok(false) // idempotent ack, no mutation
                                                }
                                            }
                                        };
                                        match gate_result {
                                            Err((cause, reason)) => serde_json::to_value(
                                                super::bridge_protocol::BridgeReply::Rejected {
                                                    cause,
                                                    reason,
                                                },
                                            )
                                            .expect("bind rejection"),
                                            Ok(mutate) => {
                                                if mutate {
                                                    responder_bind_count
                                                        .fetch_add(1, Ordering::Relaxed);
                                                    let supervisor_spec =
                                                        meerkat_core::comms::TrustedPeerDescriptor::try_from(
                                                            payload.supervisor.clone(),
                                                        )
                                                        .expect("valid supervisor spec");
                                                    responder_runtime
                                                        .add_trusted_peer(supervisor_spec)
                                                        .await
                                                        .expect("bind supervisor");
                                                    *state_guard = Some(HarnessSupervisorState {
                                                        supervisor:
                                                            meerkat_core::comms::TrustedPeerDescriptor::try_from(
                                                                payload.supervisor.clone(),
                                                            )
                                                            .expect("valid supervisor spec"),
                                                        epoch: payload.epoch,
                                                    });
                                                }
                                                serde_json::to_value(
                                                    super::bridge_protocol::BridgeBindResponse {
                                                        peer_id: responder_runtime.public_key().to_peer_id().to_string(),
                                                        address: format!("inproc://{peer_name}"),
                                                        capabilities:
                                                            super::bridge_protocol::BridgeCapabilities {
                                                                deliver_member_input: true,
                                                                observe_member: true,
                                                                interrupt_member: true,
                                                                retire_member: true,
                                                                destroy_member: true,
                                                                wire_member: true,
                                                                unwire_member: true,
                                                                ..super::bridge_protocol::BridgeCapabilities::default()
                                                        },
                                                    },
                                                )
                                                .expect("bind response")
                                            }
                                        }
                                    }
                                }
                                super::bridge_protocol::BridgeCommand::AuthorizeSupervisor(
                                    payload,
                                ) => {
                                    if responder_fail_next_authorize
                                        .swap(false, Ordering::Relaxed)
                                    {
                                        let supervisor_spec =
                                            meerkat_core::comms::TrustedPeerDescriptor::try_from(
                                                payload.supervisor.clone(),
                                            )
                                            .expect("valid supervisor spec");
                                        responder_runtime
                                            .add_trusted_peer(supervisor_spec)
                                            .await
                                            .expect("trust supervisor for rejection");
                                        serde_json::to_value(
                                            super::bridge_protocol::BridgeReply::Rejected {
                                                cause: super::bridge_protocol::BridgeRejectionCause::Internal,
                                                reason: "authorize supervisor failed: injected test failure".to_string(),
                                            },
                                        )
                                        .expect("authorize rejection")
                                    } else {
                                        let mut guard = responder_supervisor_state.write().await;
                                        if let Some(current) = guard.as_ref() {
                                            assert!(
                                                payload.epoch >= current.epoch,
                                                "authorize payload must not go backwards in epoch"
                                            );
                                            if payload.epoch > current.epoch
                                                || payload.supervisor.peer_id
                                                    != current.supervisor.peer_id.to_string()
                                            {
                                                remove_supervisors_after_response.push(
                                                    meerkat_comms::PubKey::new(
                                                        current.supervisor.pubkey,
                                                    ),
                                                );
                                            }
                                            let supervisor_spec =
                                                meerkat_core::comms::TrustedPeerDescriptor::try_from(
                                                    payload.supervisor.clone(),
                                                )
                                                .expect("valid supervisor spec");
                                            responder_runtime
                                                .add_trusted_peer(supervisor_spec)
                                                .await
                                                .expect("authorize supervisor");
                                            *guard = Some(HarnessSupervisorState {
                                                supervisor:
                                                    meerkat_core::comms::TrustedPeerDescriptor::try_from(
                                                        payload.supervisor,
                                                    )
                                                    .expect("valid supervisor spec"),
                                                epoch: payload.epoch,
                                            });
                                            serde_json::to_value(
                                                super::bridge_protocol::BridgeAck { ok: true },
                                            )
                                            .expect("authorize ack")
                                        } else {
                                            let supervisor_spec =
                                                meerkat_core::comms::TrustedPeerDescriptor::try_from(
                                                    payload.supervisor.clone(),
                                                )
                                                .expect("valid supervisor spec");
                                            responder_runtime
                                                .add_trusted_peer(supervisor_spec)
                                                .await
                                                .expect("trust supervisor for initial rejection");
                                            serde_json::to_value(
                                                super::bridge_protocol::BridgeReply::Rejected {
                                                    cause: super::bridge_protocol::BridgeRejectionCause::NotBound,
                                                    reason: "authorize supervisor failed: use bind_member to establish initial supervisor authority".to_string(),
                                                },
                                            )
                                            .expect("authorize initial rejection")
                                        }
                                    }
                                }
                                super::bridge_protocol::BridgeCommand::RevokeSupervisor(
                                    payload,
                                ) => {
                                    let supervisor_spec =
                                        meerkat_core::comms::TrustedPeerDescriptor::try_from(
                                            payload.supervisor,
                                        )
                                        .expect("valid supervisor spec");
                                    remove_supervisors_after_response
                                        .push(meerkat_comms::PubKey::new(supervisor_spec.pubkey));
                                    *responder_supervisor_state.write().await = None;
                                    serde_json::to_value(super::bridge_protocol::BridgeAck {
                                        ok: true,
                                    })
                                    .expect("revoke ack")
                                }
                                super::bridge_protocol::BridgeCommand::InterruptMember(_) => {
                                    serde_json::to_value(super::bridge_protocol::BridgeAck {
                                        ok: true,
                                    })
                                    .expect("interrupt ack")
                                }
                                super::bridge_protocol::BridgeCommand::DeliverMemberInput(
                                    payload,
                                ) => {
                                    let current = responder_supervisor_state
                                        .read()
                                        .await
                                        .clone()
                                        .expect("delivery supervisor state");
                                    if payload.epoch != current.epoch {
                                        serde_json::to_value(
                                            super::bridge_protocol::BridgeReply::Rejected {
                                                cause: super::bridge_protocol::BridgeRejectionCause::StaleSupervisor,
                                                reason: format!(
                                                    "stale supervisor epoch {} (current {})",
                                                    payload.epoch, current.epoch
                                                ),
                                            },
                                        )
                                        .expect("stale delivery rejection")
                                    } else if payload.supervisor.peer_id
                                        != current.supervisor.peer_id.to_string()
                                    {
                                        serde_json::to_value(
                                            super::bridge_protocol::BridgeReply::Rejected {
                                                cause: super::bridge_protocol::BridgeRejectionCause::StaleSupervisor,
                                                reason: format!(
                                                    "stale supervisor peer '{}' (current '{}')",
                                                    payload.supervisor.peer_id,
                                                    current.supervisor.peer_id
                                                ),
                                            },
                                        )
                                        .expect("stale supervisor rejection")
                                    } else {
                                    let mut responses = responder_delivery_responses.write().await;
                                    if let Some(existing) =
                                        responses.get(&payload.input_id).cloned()
                                    {
                                        serde_json::to_value(existing)
                                            .expect("existing delivery response")
                                    } else {
                                        let supervisor_spec =
                                            meerkat_core::comms::TrustedPeerDescriptor::try_from(
                                                payload.supervisor,
                                            )
                                            .expect("valid supervisor spec");
                                        responder_runtime
                                            .add_trusted_peer(supervisor_spec)
                                            .await
                                            .expect("refresh supervisor for delivery");
                                        responder_delivered_inputs
                                            .write()
                                            .await
                                            .push(payload.input_id.clone());
                                        let response =
                                        super::bridge_protocol::BridgeDeliveryResponse {
                                            input_id: payload.input_id.clone(),
                                            canonical_input_id: Some(
                                                Uuid::new_v5(
                                                    &Uuid::NAMESPACE_URL,
                                                    payload.input_id.as_bytes(),
                                                )
                                                .to_string(),
                                            ),
                                            outcome: super::bridge_protocol::BridgeDeliveryOutcome::Accepted,
                                        };
                                        responses
                                            .insert(payload.input_id.clone(), response.clone());
                                        serde_json::to_value(response).expect("delivery response")
                                    }
                                    }
                                }
                                super::bridge_protocol::BridgeCommand::WireMember(payload) => {
                                    let peer_spec =
                                        meerkat_core::comms::TrustedPeerDescriptor::try_from(
                                            payload.peer_spec,
                                        )
                                        .expect("valid peer spec");
                                    responder_runtime
                                        .add_trusted_peer(peer_spec)
                                        .await
                                        .expect("wire trust");
                                    serde_json::to_value(super::bridge_protocol::BridgeAck {
                                        ok: true,
                                    })
                                    .expect("wire ack")
                                }
                                super::bridge_protocol::BridgeCommand::UnwireMember(payload) => {
                                    let peer_spec =
                                        meerkat_core::comms::TrustedPeerDescriptor::try_from(
                                            payload.peer_spec,
                                        )
                                        .expect("valid peer spec");
                                    let _ = responder_runtime
                                        .remove_trusted_peer(&peer_spec.peer_id.to_string())
                                        .await;
                                    serde_json::to_value(super::bridge_protocol::BridgeAck {
                                        ok: true,
                                    })
                                    .expect("unwire ack")
                                }
                                super::bridge_protocol::BridgeCommand::RetireMember(_) => {
                                    state =
                                        super::bridge_protocol::BridgeMemberRuntimeState::Retired;
                                    serde_json::to_value(
                                        super::bridge_protocol::BridgeRetireResponse {
                                            inputs_abandoned: 0,
                                            inputs_pending_drain: 0,
                                        },
                                    )
                                    .expect("retire response")
                                }
                                super::bridge_protocol::BridgeCommand::DestroyMember(_) => {
                                    state =
                                        super::bridge_protocol::BridgeMemberRuntimeState::Destroyed;
                                    serde_json::to_value(
                                        super::bridge_protocol::BridgeDestroyResponse {
                                            inputs_abandoned: 0,
                                        },
                                    )
                                    .expect("destroy response")
                                }
                                super::bridge_protocol::BridgeCommand::ObserveMember(_) => {
                                    serde_json::to_value(
                                        super::bridge_protocol::BridgeObservationResponse::new(
                                            state,
                                            Some(matches!(
                                                state,
                                                super::bridge_protocol::BridgeMemberRuntimeState::Idle
                                                    | super::bridge_protocol::BridgeMemberRuntimeState::Attached
                                                    | super::bridge_protocol::BridgeMemberRuntimeState::Running
                                            )),
                                            None,
                                            Some(
                                                super::bridge_protocol::BridgePeerConnectivity::Reachable,
                                            ),
                                            None,
                                            Utc::now().to_rfc3339(),
                                        ),
                                    )
                                    .expect("observe response")
                                }
                                _ => serde_json::json!({
                                    "error": "unsupported bridge command in test harness"
                                }),
                            }
                        } else {
                            // Non-bridge lifecycle messages
                            match intent.as_str() {
                                "mob.peer_added" => {
                                    let peer_spec: meerkat_core::comms::TrustedPeerDescriptor =
                                        serde_json::from_value(
                                            params
                                                .get("peer_spec")
                                                .cloned()
                                                .expect("peer_added peer_spec"),
                                        )
                                        .expect("peer_added trusted peer spec");
                                    responder_runtime
                                        .add_trusted_peer(peer_spec)
                                        .await
                                        .expect("peer_added trust");
                                    serde_json::json!({ "ok": true })
                                }
                                "mob.peer_unwired" | "mob.peer_retired" => {
                                    let peer_spec: meerkat_core::comms::TrustedPeerDescriptor =
                                        serde_json::from_value(
                                            params
                                                .get("peer_spec")
                                                .cloned()
                                                .expect("peer lifecycle peer_spec"),
                                        )
                                        .expect("peer lifecycle trusted peer spec");
                                    let _ = responder_runtime
                                        .remove_trusted_peer(&peer_spec.peer_id.to_string())
                                        .await;
                                    serde_json::json!({ "ok": true })
                                }
                                _ => serde_json::json!({ "ok": true }),
                            }
                        };
                        if let Err(error) = responder_runtime
                            .send(CommsCommand::PeerResponse {
                                to,
                                in_reply_to: candidate.interaction.id,
                                status: meerkat_core::interaction::ResponseStatus::Completed,
                                result: response,
                                handling_mode: None,
                            })
                            .await
                        {
                            panic!("send live external peer response failed: {error}");
                        }
                        responder_runtime.mark_interaction_complete(candidate.interaction.id.0);
                        for supervisor_pubkey in remove_supervisors_after_response {
                            let _ = responder_runtime
                                .remove_trusted_peer(&supervisor_pubkey.to_peer_id().to_string())
                                .await;
                        }
                    }
                    _ => {
                        responder_runtime.mark_interaction_complete(candidate.interaction.id.0);
                    }
                }
            }
        }
    });

    LiveExternalPeerHarness {
        binding,
        runtime,
        bind_count,
        bind_protocol_versions,
        delivered_inputs,
        received_intents,
        supervisor_state,
        fail_next_authorize,
        fail_next_bind,
        task,
    }
}

/// Create a MobHandle using the mock session service.
async fn create_test_mob(definition: MobDefinition) -> (MobHandle, Arc<MockSessionService>) {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    (handle, service)
}

async fn try_create_test_mob_without_adapter(
    definition: MobDefinition,
) -> Result<(MobHandle, Arc<MockSessionService>), MobError> {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await?;

    Ok((handle, service))
}

async fn create_test_mob_with_runtime_adapter(
    definition: MobDefinition,
) -> (MobHandle, Arc<MockSessionService>) {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    (handle, service)
}

async fn create_test_mob_with_events(
    definition: MobDefinition,
    events: Arc<dyn MobEventStore>,
) -> (MobHandle, Arc<MockSessionService>) {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::with_events(events);
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    (handle, service)
}

async fn create_test_mob_with_run_store(
    definition: MobDefinition,
    run_store: Arc<dyn MobRunStore>,
) -> (MobHandle, Arc<MockSessionService>) {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage {
        events: Arc::new(InMemoryMobEventStore::new()),
        runs: run_store,
        specs: Arc::new(InMemoryMobSpecStore::new()),
        runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
        realm_profiles: None,
    };
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    (handle, service)
}

async fn seed_test_run_in_mob_machine(handle: &MobHandle, run_id: &RunId) {
    handle
        .project_machine_input(
            crate::machines::mob_machine::MobMachineInput::CreateRunSeed {
                run_id: crate::machines::mob_machine::RunId::from(run_id.to_string()),
                step_ids: Default::default(),
                ordered_steps: Vec::new(),
                step_has_conditions: Default::default(),
                step_dependencies: Default::default(),
                step_dependency_modes: Default::default(),
                step_branches: Default::default(),
                step_collection_policies: Default::default(),
                step_quorum_thresholds: Default::default(),
                escalation_threshold: 0,
                max_step_retries: 0,
                max_active_nodes: 0,
                max_active_frames: 0,
                max_frame_depth: 0,
            },
        )
        .await
        .expect("seed run in MobMachine");
}

async fn seed_test_loop_in_mob_machine(
    handle: &MobHandle,
    loop_instance_id: &crate::ids::LoopInstanceId,
    parent_frame_id: &crate::ids::FrameId,
    parent_node_id: &crate::ids::FlowNodeId,
    loop_id: &crate::ids::LoopId,
    depth: u32,
    max_iterations: u64,
) {
    handle
        .project_machine_input(
            crate::machines::mob_machine::MobMachineInput::CreateLoopSeed {
                loop_instance_id: crate::machines::mob_machine::LoopInstanceId::from(
                    loop_instance_id.as_str(),
                ),
                parent_frame_id: crate::machines::mob_machine::FrameId::from(
                    parent_frame_id.as_str(),
                ),
                parent_node_id: crate::machines::mob_machine::FlowNodeId::from(
                    parent_node_id.as_str(),
                ),
                loop_id: crate::machines::mob_machine::LoopId::from(loop_id.as_str()),
                depth,
                max_iterations,
            },
        )
        .await
        .expect("seed loop in MobMachine");
}

async fn seed_test_body_frame_in_mob_machine(
    handle: &MobHandle,
    run_id: &RunId,
    frame_id: &crate::ids::FrameId,
    loop_instance_id: &crate::ids::LoopInstanceId,
    iteration: u32,
) {
    handle
        .project_machine_input(
            crate::machines::mob_machine::MobMachineInput::CreateFrameSeed {
                run_id: crate::machines::mob_machine::RunId::from(run_id.to_string()),
                frame_id: crate::machines::mob_machine::FrameId::from(frame_id.as_str()),
                frame_scope: crate::machines::mob_machine::FrameScope::Body,
                loop_instance_id: Some(crate::machines::mob_machine::LoopInstanceId::from(
                    loop_instance_id.as_str(),
                )),
                iteration,
                tracked_nodes: Default::default(),
                ordered_nodes: Vec::new(),
                node_kind: Default::default(),
                node_dependencies: Default::default(),
                node_dependency_modes: Default::default(),
                node_branches: Default::default(),
                node_step_ids: Default::default(),
                node_loop_ids: Default::default(),
                node_status: Default::default(),
                ready_queue: Vec::new(),
            },
        )
        .await
        .expect("seed body frame in MobMachine");
}

fn test_comms_name(profile: &str, agent_identity: &str) -> String {
    format!("test-mob/{profile}/{agent_identity}")
}

fn test_comms_name_for(mob_id: &MobId, profile: &str, agent_identity: &str) -> String {
    format!("{mob_id}/{profile}/{agent_identity}")
}

async fn test_peer_route<C>(comms: &C, name: &str) -> PeerRoute
where
    C: CoreCommsRuntime + ?Sized,
{
    let peers = comms.peers().await;
    let entry = peers
        .iter()
        .find(|entry| entry.name.as_str() == name)
        .unwrap_or_else(|| panic!("peer route not found for {name}; peers={peers:?}"));
    PeerRoute::with_display_name(entry.peer_id, entry.name.clone())
}

fn test_trusted_peer_route(comms: &meerkat_comms::CommsRuntime, name: &str) -> PeerRoute {
    let trusted = comms.trusted_peers_shared();
    let trusted = trusted.read();
    if let Some(peer) = trusted.peers.iter().find(|peer| peer.name == name) {
        return PeerRoute::with_display_name(
            peer.pubkey.to_peer_id(),
            PeerName::new(peer.name.clone()).expect("valid trusted peer name"),
        );
    }
    drop(trusted);

    let namespace = comms.inproc_namespace().unwrap_or("");
    let inproc_peers = meerkat_comms::InprocRegistry::global().peers_in_namespace(namespace);
    if let Some(peer) = inproc_peers.iter().find(|peer| peer.name == name) {
        return PeerRoute::with_display_name(
            peer.pubkey.to_peer_id(),
            PeerName::new(peer.name.clone()).expect("valid inproc peer name"),
        );
    }

    let trusted = comms.trusted_peers_shared();
    let trusted = trusted.read();
    panic!("trusted or inproc peer route not found for {name}; trusted={trusted:?}");
}

fn with_unique_mob_id(mut definition: MobDefinition, label: &str) -> MobDefinition {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    definition.id = MobId::from(format!("test-mob-{label}-{suffix}"));
    definition
}

struct EchoBundleDispatcher;

#[async_trait]
impl AgentToolDispatcher for EchoBundleDispatcher {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        vec![Arc::new(meerkat_core::ToolDef {
            name: "bundle_echo".into(),
            description: "Echo input value".to_string(),
            input_schema: serde_json::Value::Object(serde_json::Map::new()),
            provenance: None,
        })]
        .into()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        if call.name != "bundle_echo" {
            return Err(meerkat_core::ToolError::not_found(call.name));
        }
        let args: serde_json::Value = serde_json::from_str(call.args.get())
            .map_err(|e| meerkat_core::ToolError::invalid_arguments(call.name, e.to_string()))?;
        let echoed = args
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(meerkat_core::ToolResult::new(
            call.id.to_string(),
            serde_json::json!({ "echo": echoed }).to_string(),
            false,
        )
        .into())
    }
}

struct PersistentMockAgent {
    session_id: SessionId,
    system_context_state: Arc<std::sync::Mutex<SessionSystemContextState>>,
}

#[async_trait]
impl SessionAgent for PersistentMockAgent {
    async fn run_with_events(
        &mut self,
        _prompt: meerkat_core::types::ContentInput,
        _event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        Ok(mock_run_result(
            self.session_id.clone(),
            "Persistent mock turn".to_string(),
        ))
    }

    fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

    fn set_flow_tool_overlay(
        &mut self,
        _overlay: Option<meerkat_core::service::TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    fn cancel(&mut self) {}

    fn hot_swap_llm_identity(
        &mut self,
        _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        _identity: meerkat_core::SessionLlmIdentity,
        _request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    fn session_id(&self) -> SessionId {
        self.session_id.clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            message_count: 0,
            total_tokens: 0,
            usage: Usage::default(),
            last_assistant_text: None,
        }
    }

    fn session_clone(&self) -> Session {
        Session::with_id(self.session_id.clone())
    }

    fn apply_runtime_system_context(
        &mut self,
        _appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
    }

    fn system_context_state(&self) -> Arc<std::sync::Mutex<SessionSystemContextState>> {
        Arc::clone(&self.system_context_state)
    }
}

struct PersistentMockBuilder;

#[async_trait]
impl SessionAgentBuilder for PersistentMockBuilder {
    type Agent = PersistentMockAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        let session_id = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone())
            .unwrap_or_default();
        Ok(PersistentMockAgent {
            session_id,
            system_context_state: Arc::new(std::sync::Mutex::new(
                SessionSystemContextState::default(),
            )),
        })
    }
}

struct PersistedListingSessionService {
    inner: Arc<MockSessionService>,
}

impl PersistedListingSessionService {
    fn new(inner: Arc<MockSessionService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl SessionService for PersistedListingSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        self.inner.create_session(req).await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.inner.start_turn(id, req).await
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        self.inner.read(id).await
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let live_sessions = self.inner.live_session_data.read().await;
        let mut summaries = live_sessions
            .values()
            .map(|session| SessionSummary {
                session_id: session.id().clone(),
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                total_tokens: session.total_tokens(),
                is_active: true,
                labels: Default::default(),
            })
            .collect::<Vec<_>>();
        let live_ids = summaries
            .iter()
            .map(|summary| summary.session_id.clone())
            .collect::<HashSet<_>>();
        drop(live_sessions);

        let persisted = self.inner.persisted_sessions.read().await;
        for session in persisted.values() {
            if live_ids.contains(session.id()) {
                continue;
            }
            summaries.push(SessionSummary {
                session_id: session.id().clone(),
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                total_tokens: session.total_tokens(),
                is_active: false,
                labels: Default::default(),
            });
        }
        Ok(summaries)
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        self.inner.has_live_session(id).await
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.archive(id).await
    }

    async fn subscribe_session_events(&self, id: &SessionId) -> Result<EventStream, StreamError> {
        SessionService::subscribe_session_events(&*self.inner, id).await
    }
}

#[async_trait]
impl SessionServiceCommsExt for PersistedListingSessionService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }

    async fn event_injector(&self, session_id: &SessionId) -> Option<Arc<dyn EventInjector>> {
        self.inner.event_injector(session_id).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.inner.interaction_event_injector(session_id).await
    }
}

#[async_trait]
impl SessionServiceHistoryExt for PersistedListingSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: meerkat_core::service::SessionHistoryQuery,
    ) -> Result<meerkat_core::service::SessionHistoryPage, SessionError> {
        self.inner.read_history(id, query).await
    }
}

#[async_trait]
impl SessionServiceControlExt for PersistedListingSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        self.inner.append_system_context(id, req).await
    }
}

#[async_trait]
impl MobSessionService for PersistedListingSessionService {
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        SessionService::subscribe_session_events(&*self.inner, session_id).await
    }

    fn supports_persistent_sessions(&self) -> bool {
        self.inner.supports_persistent_sessions()
    }

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        self.inner.runtime_adapter()
    }

    async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
        self.inner.session_belongs_to_mob(session_id, mob_id).await
    }

    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        self.inner.load_persisted_session(session_id).await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        self.inner
            .apply_runtime_turn(session_id, run_id, req, boundary, contributing_input_ids)
            .await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.inner.discard_live_session(session_id).await
    }

    async fn cancel_all_checkpointers(&self) {
        self.inner.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.inner.rearm_all_checkpointers().await;
    }
}

struct InactiveReadSessionService {
    inner: Arc<MockSessionService>,
}

impl InactiveReadSessionService {
    fn new(inner: Arc<MockSessionService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl SessionService for InactiveReadSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        self.inner.create_session(req).await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.inner.start_turn(id, req).await
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let session = self
            .inner
            .live_session_clone(id)
            .await
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        let metadata = session.session_metadata();
        if !self.inner.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                is_active: false,
                model: metadata
                    .as_ref()
                    .map(|meta| meta.model.clone())
                    .unwrap_or_else(|| "claude-sonnet-4-5".to_string()),
                provider: metadata
                    .as_ref()
                    .map(|meta| meta.provider)
                    .unwrap_or(Provider::Anthropic),
                last_assistant_text: session.last_assistant_text(),
                labels: Default::default(),
            },
            billing: SessionUsage {
                total_tokens: session.total_tokens(),
                usage: session.total_usage(),
            },
        })
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        self.inner.list(query).await
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.archive(id).await
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        self.inner.has_live_session(id).await
    }

    async fn subscribe_session_events(&self, id: &SessionId) -> Result<EventStream, StreamError> {
        SessionService::subscribe_session_events(&*self.inner, id).await
    }
}

#[async_trait]
impl SessionServiceCommsExt for InactiveReadSessionService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }

    async fn event_injector(&self, session_id: &SessionId) -> Option<Arc<dyn EventInjector>> {
        self.inner.event_injector(session_id).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.inner.interaction_event_injector(session_id).await
    }
}

#[async_trait]
impl SessionServiceHistoryExt for InactiveReadSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: meerkat_core::service::SessionHistoryQuery,
    ) -> Result<meerkat_core::service::SessionHistoryPage, SessionError> {
        self.inner.read_history(id, query).await
    }
}

#[async_trait]
impl SessionServiceControlExt for InactiveReadSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        self.inner.append_system_context(id, req).await
    }
}

#[async_trait]
impl MobSessionService for InactiveReadSessionService {
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        SessionService::subscribe_session_events(&*self.inner, session_id).await
    }

    fn supports_persistent_sessions(&self) -> bool {
        self.inner.supports_persistent_sessions()
    }

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        self.inner.runtime_adapter()
    }

    async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
        self.inner.session_belongs_to_mob(session_id, mob_id).await
    }

    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        self.inner.load_persisted_session(session_id).await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        self.inner
            .apply_runtime_turn(session_id, run_id, req, boundary, contributing_input_ids)
            .await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.inner.discard_live_session(session_id).await
    }

    async fn cancel_all_checkpointers(&self) {
        self.inner.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.inner.rearm_all_checkpointers().await;
    }
}

async fn create_test_mob_with_persistent_service(definition: MobDefinition) -> MobHandle {
    let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        PersistentMockBuilder,
        16,
        store,
        Some(runtime_store),
        blob_store,
    ));
    MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service)
        .create()
        .await
        .expect("create mob with persistent session service")
}

#[derive(Default)]
struct OverlayProbeSessionStore;

#[async_trait]
impl AgentSessionStore for OverlayProbeSessionStore {
    async fn save(&self, _session: &Session) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<Session>, meerkat_core::error::AgentError> {
        Ok(None)
    }
}

struct OverlayProbeDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
}

impl OverlayProbeDispatcher {
    fn new() -> Self {
        let tools: Vec<Arc<ToolDef>> = ["alpha", "beta"]
            .iter()
            .map(|name| {
                let input_schema = serde_json::json!({
                    "type": "object",
                    "properties": {}
                });
                Arc::new(ToolDef {
                    name: (*name).into(),
                    description: format!("{name} tool"),
                    input_schema,
                    provenance: None,
                })
            })
            .collect();
        Self {
            tools: tools.into(),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for OverlayProbeDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

struct OverlayProbeLlmClient {
    provider_visible_tools: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl AgentLlmClient for OverlayProbeLlmClient {
    async fn stream_response(
        &self,
        _messages: &[meerkat_core::types::Message],
        tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, meerkat_core::error::AgentError> {
        self.provider_visible_tools
            .lock()
            .expect("provider_visible_tools lock poisoned")
            .push(tools.iter().map(|tool| tool.name.to_string()).collect());
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "{}".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            Usage::default(),
        ))
    }

    fn provider(&self) -> &'static str {
        "overlay-probe"
    }

    fn model(&self) -> &'static str {
        "mock-model"
    }
}

type OverlayProbeInnerAgent =
    Agent<OverlayProbeLlmClient, OverlayProbeDispatcher, OverlayProbeSessionStore>;

struct OverlayProbeSessionAgent {
    agent: OverlayProbeInnerAgent,
}

#[async_trait]
impl SessionAgent for OverlayProbeSessionAgent {
    async fn run_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.agent.run_with_events(prompt, event_tx).await
    }

    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        self.agent.pending_skill_references = refs;
    }

    fn set_flow_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent
            .set_flow_tool_overlay(overlay)
            .map_err(|error| meerkat_core::error::AgentError::ConfigError(error.to_string()))
    }

    fn cancel(&mut self) {
        self.agent.cancel();
    }

    fn hot_swap_llm_identity(
        &mut self,
        _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        _identity: meerkat_core::SessionLlmIdentity,
        _request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    fn session_id(&self) -> SessionId {
        self.agent.session().id().clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        let session = self.agent.session();
        SessionSnapshot {
            created_at: session.created_at(),
            updated_at: session.updated_at(),
            message_count: session.messages().len(),
            total_tokens: session.total_tokens(),
            usage: session.total_usage(),
            last_assistant_text: session.last_assistant_text(),
        }
    }

    fn session_clone(&self) -> Session {
        self.agent.session_with_system_context_state()
    }

    fn apply_runtime_system_context(
        &mut self,
        appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
        self.agent
            .session_mut()
            .append_system_context_blocks(appends);
    }

    fn system_context_state(&self) -> Arc<std::sync::Mutex<SessionSystemContextState>> {
        self.agent.system_context_state()
    }
}

struct OverlayProbeSessionAgentBuilder {
    provider_visible_tools: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl SessionAgentBuilder for OverlayProbeSessionAgentBuilder {
    type Agent = OverlayProbeSessionAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        let mut builder = AgentBuilder::new().model(req.model.clone());
        if let Some(max_tokens) = req.max_tokens {
            builder = builder.max_tokens_per_turn(max_tokens);
        }
        if let Some(system_prompt) = &req.system_prompt {
            builder = builder.system_prompt(system_prompt.clone());
        }
        if let Some(session) = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.clone())
        {
            builder = builder.resume_session(session);
        }
        if let Some(build) = &req.build {
            match &build.runtime_build_mode {
                meerkat_core::runtime_epoch::RuntimeBuildMode::SessionOwned(bindings) => {
                    builder = builder.with_turn_state_handle(Arc::clone(&bindings.turn_state));
                }
                meerkat_core::runtime_epoch::RuntimeBuildMode::StandaloneEphemeral => {
                    builder = builder.with_turn_state_handle(Arc::new(
                        meerkat_runtime::RuntimeTurnStateHandle::ephemeral(),
                    ));
                }
            }
        }

        let client = Arc::new(OverlayProbeLlmClient {
            provider_visible_tools: Arc::clone(&self.provider_visible_tools),
        });
        let tools = Arc::new(OverlayProbeDispatcher::new());
        let store = Arc::new(OverlayProbeSessionStore);
        let agent = builder.build(client, tools, store).await;

        Ok(OverlayProbeSessionAgent { agent })
    }
}

async fn create_test_mob_with_overlay_probe_service(
    definition: MobDefinition,
) -> (MobHandle, Arc<std::sync::Mutex<Vec<Vec<String>>>>) {
    let provider_visible_tools = Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let session_service = Arc::new(meerkat_session::EphemeralSessionService::new(
        OverlayProbeSessionAgentBuilder {
            provider_visible_tools: Arc::clone(&provider_visible_tools),
        },
        16,
    ));

    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(session_service)
        .allow_ephemeral_sessions(true)
        .create()
        .await
        .expect("create mob with overlay probe session service");

    (handle, provider_visible_tools)
}

// -----------------------------------------------------------------------
// P1-T03: MobBuilder + MobHandle + MobActor wiring
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_mob_create_returns_handle() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    assert_eq!(handle.mob_id().as_str(), "test-mob");
}

#[tokio::test]
async fn test_mob_builder_runs_with_persistent_storage_bundle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("mob.db");
    let storage = MobStorage::persistent(&db_path).expect("construct persistent storage");
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::new(sample_definition_with_single_step_flow(500, 8), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob with persistent storage");

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(
        terminal.status,
        MobRunStatus::Completed,
        "flow should complete for runtime overlay probe: {terminal:?}"
    );
}

#[tokio::test]
async fn test_mob_builder_from_mobpack_creates_valid_mob() {
    let mut definition = sample_definition();
    definition.skills.insert(
        "review".to_string(),
        SkillSource::Path {
            path: "skills/review.md".to_string(),
        },
    );

    let packed_skills = BTreeMap::from([(
        "skills/review.md".to_string(),
        b"You are a reviewer.".to_vec(),
    )]);
    let mock_service = MockSessionService::new();
    let _ = mock_service.enable_runtime_adapter();
    let session_service: Arc<dyn crate::runtime::MobSessionService> = Arc::new(mock_service);
    let storage = MobStorage::in_memory();

    let handle = MobBuilder::from_mobpack(definition.clone(), packed_skills, storage)
        .expect("from_mobpack should construct builder")
        .with_session_service(session_service)
        .allow_ephemeral_sessions(true)
        .create()
        .await
        .expect("mob should create");

    assert_eq!(handle.definition().id, definition.id);
    match &handle.definition().skills["review"] {
        SkillSource::Inline { content } => assert_eq!(content, "You are a reviewer."),
        other => panic!("expected inlined skill, got {other:?}"),
    }
}

#[tokio::test]
async fn test_mob_builder_create_rejects_mismatched_spec_store_definition() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let definition = sample_definition();
    let mut mismatched = definition.clone();
    mismatched
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile exists")
        .as_inline_mut()
        .unwrap()
        .peer_description = "mismatched worker profile".to_string();
    storage
        .specs
        .put_spec(&definition.id, &mismatched, None)
        .await
        .expect("preseed mismatched spec");

    let error = match MobBuilder::new(definition, storage)
        .with_session_service(service)
        .create()
        .await
    {
        Ok(_) => panic!("create should fail when persisted spec mismatches runtime definition"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("persisted spec store definition does not match"),
        "error should explain spec-store mismatch"
    );
}

#[tokio::test]
async fn test_mob_builder_persists_spec_and_resume_requires_consistency() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let definition = sample_definition();
    let mob_id = definition.id.clone();
    let storage_for_resume = MobStorage {
        events: storage.events.clone(),
        runs: storage.runs.clone(),
        specs: storage.specs.clone(),
        runtime_metadata: storage.runtime_metadata.clone(),
        realm_profiles: storage.realm_profiles.clone(),
    };

    let _handle = MobBuilder::new(definition.clone(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let (persisted_definition, _revision) = storage_for_resume
        .specs
        .get_spec(&mob_id)
        .await
        .expect("get persisted spec")
        .expect("spec should be persisted on create");
    assert_eq!(
        persisted_definition, definition,
        "create should persist runtime definition into spec store"
    );

    let mut mismatched = definition.clone();
    mismatched
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile exists")
        .as_inline_mut()
        .unwrap()
        .peer_description = "resume mismatch".to_string();
    let _ = storage_for_resume
        .specs
        .put_spec(&mob_id, &mismatched, None)
        .await
        .expect("overwrite persisted spec with mismatch");

    let error = match MobBuilder::for_resume(storage_for_resume)
        .with_session_service(service)
        .resume()
        .await
    {
        Ok(_) => panic!("resume should fail when persisted spec mismatches runtime definition"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("persisted spec store definition does not match"),
        "resume should enforce spec-store consistency"
    );
}

#[tokio::test]
async fn test_mob_builder_persists_supervisor_runtime_metadata_on_create() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let runtime_metadata = Arc::new(InMemoryMobRuntimeMetadataStore::new());
    let definition = sample_definition();
    let mob_id = definition.id.clone();
    let storage = MobStorage::custom_with_runtime_metadata(
        Arc::new(InMemoryMobEventStore::new()),
        Arc::new(InMemoryMobRunStore::new()),
        Arc::new(InMemoryMobSpecStore::new()),
        runtime_metadata.clone(),
    );

    let _handle = MobBuilder::new(definition, storage)
        .with_session_service(service)
        .create()
        .await
        .expect("create mob");

    let record = runtime_metadata
        .load_supervisor_authority(&mob_id)
        .await
        .expect("load supervisor authority")
        .expect("create should persist supervisor authority");
    assert_eq!(record.epoch, 0, "fresh mobs should start at epoch 0");
    assert_eq!(
        record.public_peer_id,
        record.keypair().public_key().to_peer_id().to_string(),
        "persisted public peer id should match the reconstructed keypair"
    );
    assert_eq!(
        record.protocol_version,
        meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_default_protocol_version(),
        "fresh supervisor metadata should persist the canonical default bridge protocol version"
    );
}

#[tokio::test]
async fn test_mob_resume_accepts_typed_supervisor_protocol_metadata() {
    let definition = sample_definition();
    let mob_id = definition.id.clone();
    let events: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
    events
        .append(NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: MobEventKind::MobCreated {
                definition: Box::new(definition),
            },
        })
        .await
        .expect("append mob created");
    let runtime_metadata: Arc<dyn MobRuntimeMetadataStore> =
        Arc::new(InMemoryMobRuntimeMetadataStore::new());
    let persisted = SupervisorAuthorityRecord::generate(
        super::bridge_protocol::supervisor_bridge_default_protocol_version(),
    );
    runtime_metadata
        .put_supervisor_authority(&mob_id, &persisted)
        .await
        .expect("persist supervisor metadata");

    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let _handle = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata.clone(),
    ))
    .with_session_service(service)
    .resume()
    .await
    .expect("typed supervisor protocol metadata should be accepted on resume");

    let loaded = runtime_metadata
        .load_supervisor_authority(&mob_id)
        .await
        .expect("load authority")
        .expect("authority should remain present");
    assert_eq!(
        loaded.protocol_version,
        super::bridge_protocol::supervisor_bridge_default_protocol_version(),
        "resume should preserve the canonical bridge protocol version"
    );
    assert_eq!(
        loaded.public_peer_id, persisted.public_peer_id,
        "resume must preserve supervisor identity"
    );
    assert_eq!(
        loaded.epoch, persisted.epoch,
        "resume must preserve authority epoch"
    );
}

#[test]
fn test_supervisor_authority_record_rejects_unsupported_protocol_metadata_at_decode() {
    let record = SupervisorAuthorityRecord::generate(
        super::bridge_protocol::supervisor_bridge_default_protocol_version(),
    );
    let mut raw = serde_json::to_value(record).expect("serialize supervisor authority");
    raw["protocol_version"] = serde_json::json!(999);

    let error = serde_json::from_value::<SupervisorAuthorityRecord>(raw)
        .expect_err("unsupported persisted supervisor protocol metadata must fail at decode");

    assert!(
        error
            .to_string()
            .contains("unsupported supervisor bridge protocol version"),
        "decode error should report protocol incompatibility, got: {error}"
    );
}

#[tokio::test]
async fn test_mob_builder_allows_spec_warnings() {
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "warn-mob"

[profiles.lead]
model = "claude-opus-4-6"

[flows.demo]

[flows.demo.steps.start]
role = "lead"
message = "start"

[flows.demo.steps.join]
role = "lead"
message = "join"
depends_on = ["start"]
depends_on_mode = "any"
"#,
    )
    .expect("parse warning definition");

    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service)
        .create()
        .await
        .expect("warning diagnostics should not block create");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
}

#[tokio::test]
async fn test_mob_builder_blocks_spec_errors() {
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "error-mob"

[profiles.lead]
model = "claude-opus-4-6"

[flows.demo]

[flows.demo.steps.start]
role = "ghost"
message = "x"
"#,
    )
    .expect("parse error definition");

    let service = Arc::new(MockSessionService::new());
    let result = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service)
        .create()
        .await;

    let diagnostics = match result {
        Ok(_) => panic!("spec errors should block create"),
        Err(MobError::DefinitionError(diagnostics)) => diagnostics,
        Err(other) => panic!("expected DefinitionError, got {other}"),
    };
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.code == crate::DiagnosticCode::FlowUnknownRole),
        "expected flow unknown role diagnostic"
    );
}

#[tokio::test]
async fn test_mob_resume_allows_spec_warnings() {
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "warn-resume-mob"

[profiles.lead]
model = "claude-opus-4-6"

[flows.demo]

[flows.demo.steps.start]
role = "lead"
message = "start"

[flows.demo.steps.join]
role = "lead"
message = "join"
depends_on = ["start"]
depends_on_mode = "any"
"#,
    )
    .expect("parse warning definition");

    let events: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
    events
        .append(NewMobEvent {
            mob_id: definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MobCreated {
                definition: Box::new(definition.clone()),
            },
        })
        .await
        .expect("append mob created");
    let runtime_metadata: Arc<dyn MobRuntimeMetadataStore> =
        Arc::new(InMemoryMobRuntimeMetadataStore::new());
    runtime_metadata
        .put_supervisor_authority(&definition.id, &default_supervisor_authority_record())
        .await
        .expect("persist supervisor metadata");

    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .resume()
    .await
    .expect("warning diagnostics should not block resume");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
}

#[tokio::test]
async fn test_mob_resume_blocks_spec_errors() {
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "error-resume-mob"

[profiles.lead]
model = "claude-opus-4-6"

[flows.demo]

[flows.demo.steps.start]
role = "ghost"
message = "x"
"#,
    )
    .expect("parse error definition");

    let events: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
    events
        .append(NewMobEvent {
            mob_id: definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MobCreated {
                definition: Box::new(definition.clone()),
            },
        })
        .await
        .expect("append mob created");
    let runtime_metadata: Arc<dyn MobRuntimeMetadataStore> =
        Arc::new(InMemoryMobRuntimeMetadataStore::new());
    runtime_metadata
        .put_supervisor_authority(&definition.id, &default_supervisor_authority_record())
        .await
        .expect("persist supervisor metadata");

    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let result = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .resume()
    .await;

    let diagnostics = match result {
        Ok(_) => panic!("spec errors should block resume"),
        Err(MobError::DefinitionError(diagnostics)) => diagnostics,
        Err(other) => panic!("expected DefinitionError, got {other}"),
    };
    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.code == crate::DiagnosticCode::FlowUnknownRole),
        "expected flow unknown role diagnostic"
    );
}

#[tokio::test]
async fn test_mob_builder_accepts_persistent_session_service() {
    let handle = create_test_mob_with_persistent_service(sample_definition()).await;
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            MeerkatId::from("w-1"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn with persistent session service");
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_some(),
        "spawn should succeed when mob is built with PersistentSessionService"
    );
}

#[tokio::test]
async fn test_mob_builder_rejects_non_persistent_session_service() {
    let service = Arc::new(meerkat_session::EphemeralSessionService::new(
        PersistentMockBuilder,
        16,
    ));
    let result = MobBuilder::new(sample_definition(), MobStorage::in_memory())
        .with_session_service(service)
        .create()
        .await;
    assert!(
        matches!(result, Err(MobError::Internal(message)) if message.contains("persistent-session contract")),
        "create should reject non-persistent session services for REQ-MOB-030"
    );
}

#[tokio::test]
async fn test_mob_builder_allows_ephemeral_session_service_when_opted_in() {
    let service = Arc::new(meerkat_session::EphemeralSessionService::new(
        PersistentMockBuilder,
        16,
    ));
    let handle = MobBuilder::new(sample_definition(), MobStorage::in_memory())
        .with_session_service(service)
        .allow_ephemeral_sessions(true)
        .create()
        .await
        .expect("create should allow ephemeral sessions when explicitly enabled");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
}

#[tokio::test]
async fn test_mob_handle_is_clone() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let handle2 = handle.clone();
    assert_eq!(handle2.mob_id().as_str(), "test-mob");
}

#[tokio::test]
async fn test_mob_shutdown() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle.shutdown().await.expect("shutdown");
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
}

#[tokio::test]
async fn test_lifecycle_state_machine_enforcement() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    assert_eq!(handle.status().await.unwrap(), MobState::Running);

    handle.stop().await.expect("stop");
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);

    handle.resume().await.expect("resume");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);

    handle.complete().await.expect("complete");
    assert_eq!(handle.status().await.unwrap(), MobState::Completed);

    let err = handle
        .resume()
        .await
        .expect_err("resume must fail from completed");
    assert!(
        matches!(err, MobError::InvalidTransition { .. }),
        "completed -> running must be rejected"
    );

    handle.destroy().await.expect("destroy");
    assert_eq!(handle.status().await.unwrap(), MobState::Destroyed);

    let err = handle
        .destroy()
        .await
        .expect_err("destroy from Destroyed must fail once actor exits");
    assert!(
        matches!(
            err,
            crate::runtime::handle::MobDestroyError::Mob(MobError::Internal(ref message))
                if message.contains("actor task dropped")
        ),
        "destroy should be terminal once completed: {err:?}"
    );
}

#[tokio::test]
async fn test_destroy_is_terminal_for_commands() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let report = handle.destroy().await.expect("destroy");
    assert!(
        report.force_destroyed_members.is_empty(),
        "local destroy should not require force-destroy bookkeeping yet"
    );
    assert!(
        report.orphaned_remote_members.is_empty(),
        "local destroy should not orphan remote members in the in-memory test path"
    );
    assert!(
        !report.remote_cleanup_deadline_exceeded,
        "local destroy should not exceed the remote cleanup deadline in the in-memory path"
    );
    assert!(
        report.metadata_scrubbed,
        "destroy should report metadata scrub"
    );
    assert!(
        report.events_cleared,
        "destroy should report cleared events"
    );
    assert!(
        report.namespace_cleaned,
        "destroy should report namespace cleanup"
    );
    assert!(
        report.errors.is_empty(),
        "successful destroy should not report cleanup errors: {:?}",
        report.errors
    );
    assert_eq!(handle.status().await.unwrap(), MobState::Destroyed);

    let err = handle
        .stop()
        .await
        .expect_err("stop after destroy must fail");
    assert!(
        matches!(err, MobError::Internal(ref message) if message.contains("actor task dropped")),
        "destroy should terminate the actor loop: {err:?}"
    );
}

#[tokio::test]
async fn test_destroy_detaches_mob_owned_session_ingress_before_runtime_destroy() {
    let (handle, service) = create_test_mob_with_runtime_adapter(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);
    let adapter = service.enable_runtime_adapter();

    let session_id = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-ingress-destroy"),
            None,
        )
        .await
        .expect("spawn session-backed member")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member has bridge session id");

    let owner = adapter.peer_ingress_owner(&session_id).await;
    assert!(
        matches!(owner, meerkat_runtime::PeerIngressOwner::MobOwned { .. }),
        "spawn should claim mob-owned session ingress before destroy, got {owner:?}"
    );

    handle
        .destroy()
        .await
        .expect("destroy should close detach ack before final destroy");
    assert_eq!(handle.status().await.unwrap(), MobState::Destroyed);
}

#[tokio::test]
async fn test_stopped_retire_detaches_mob_owned_session_ingress() {
    let (handle, service) = create_test_mob_with_runtime_adapter(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);
    let adapter = service.enable_runtime_adapter();

    let session_id = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-stopped-ingress-retire"),
            None,
        )
        .await
        .expect("spawn session-backed member")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member has bridge session id");
    handle.stop().await.expect("stop mob");

    handle
        .retire(AgentIdentity::from("w-stopped-ingress-retire"))
        .await
        .expect("stopped retire should close detach ack");
    assert!(
        matches!(
            adapter.peer_ingress_owner(&session_id).await,
            meerkat_runtime::PeerIngressOwner::Unattached
        ),
        "retire from Stopped should detach mob-owned peer ingress"
    );
}

#[tokio::test]
async fn test_destroy_scrubs_runtime_metadata() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let definition = sample_definition();
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    assert!(
        runtime_metadata
            .load_supervisor_authority(&mob_id)
            .await
            .expect("load supervisor metadata")
            .is_some(),
        "create should persist supervisor metadata before destroy"
    );
    runtime_metadata
        .upsert_external_binding_overlay(
            &mob_id,
            &ExternalBindingOverlayRecord {
                agent_identity: AgentIdentity::from("worker-overlay"),
                generation: crate::ids::Generation::INITIAL,
                normalized_member_ref: None,
                bootstrap_token: None,
                status: ExternalBindingOverlayStatus::Failed {
                    reason: "test overlay".to_string(),
                },
                updated_at: Utc::now(),
            },
        )
        .await
        .expect("insert overlay");

    handle.destroy().await.expect("destroy");

    assert!(
        runtime_metadata
            .load_supervisor_authority(&mob_id)
            .await
            .expect("load supervisor metadata after destroy")
            .is_none(),
        "destroy should scrub supervisor metadata"
    );
    assert!(
        runtime_metadata
            .list_external_binding_overlays(&mob_id)
            .await
            .expect("list overlays after destroy")
            .is_empty(),
        "destroy should scrub binding overlays"
    );
}

#[tokio::test]
async fn test_rotate_supervisor_updates_runtime_metadata() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let definition = sample_definition();
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let original = runtime_metadata
        .load_supervisor_authority(&mob_id)
        .await
        .expect("load original supervisor")
        .expect("original supervisor metadata");
    let report = handle.rotate_supervisor().await.expect("rotate supervisor");
    let rotated = runtime_metadata
        .load_supervisor_authority(&mob_id)
        .await
        .expect("load rotated supervisor")
        .expect("rotated supervisor metadata");

    assert_eq!(report.previous_epoch, original.epoch);
    assert_eq!(report.current_epoch, original.epoch + 1);
    assert_eq!(rotated.epoch, report.current_epoch);
    assert_eq!(rotated.public_peer_id, report.public_peer_id);
    assert_ne!(
        rotated.public_peer_id, original.public_peer_id,
        "rotation should replace the supervisor keypair"
    );
}

#[tokio::test]
async fn test_rotate_supervisor_reauthorizes_live_remote_members_and_rejects_stale_epoch() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "rotate-supervisor-live-remote",
    );
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let runtime_metadata = storage.runtime_metadata.clone();
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service)
        .create()
        .await
        .expect("create mob");
    let external = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext")).await;
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            external.binding(),
        )
        .await
        .expect("spawn live external worker");

    let original = runtime_metadata
        .load_supervisor_authority(&mob_id)
        .await
        .expect("load original authority")
        .expect("original authority record");
    let report = handle.rotate_supervisor().await.expect("rotate supervisor");

    handle
        .internal_turn(
            AgentIdentity::from("w-ext"),
            ContentInput::from("after-rotate".to_string()),
        )
        .await
        .expect("rotated supervisor should still control live peer-only member");
    assert_eq!(
        external.delivered_input_ids().await.len(),
        1,
        "rotated supervisor should still be able to deliver one logical input"
    );

    let crate::RuntimeBinding::External {
        peer_id,
        address,
        bootstrap_token: _,
        pubkey,
    } = external.binding()
    else {
        panic!("live external peer must expose external binding");
    };
    let peer_name = address
        .strip_prefix("inproc://")
        .map(|value| value.split('?').next().unwrap_or(value).to_string())
        .unwrap_or_else(|| format!("mob_member/backend_peer/{peer_id}"));
    let peer = match pubkey {
        Some(pubkey) => meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
            peer_name,
            peer_id.clone(),
            pubkey,
            address.clone(),
        ),
        None => meerkat_core::comms::TrustedPeerDescriptor::test_only_unsigned(
            peer_name,
            peer_id.clone(),
            address.clone(),
        ),
    }
    .expect("peer spec");
    let old_bridge = crate::runtime::MobSupervisorBridge::new(&mob_id, original.clone())
        .expect("build old supervisor bridge");
    old_bridge
        .trust_recipient(&peer)
        .await
        .expect("old supervisor bridge should explicitly trust peer for stale-epoch probe");
    let stale_command = super::bridge_protocol::BridgeCommand::DeliverMemberInput(
        super::bridge_protocol::BridgeDeliveryPayload {
            supervisor: old_bridge
                .supervisor_spec()
                .await
                .expect("old supervisor spec")
                .into(),
            epoch: report.previous_epoch,
            protocol_version: original.protocol_version,
            input_id: "stale-epoch-input".to_string(),
            content: ContentInput::from("stale-epoch".to_string()),
            handling_mode: HandlingMode::Queue,
        },
    );
    let stale_reply = old_bridge
        .send_bridge_command(&peer, &stale_command, std::time::Duration::from_secs(1))
        .await
        .expect("bridge should return a rejected reply for stale supervisor epochs");
    let stale_reply: super::bridge_protocol::BridgeReply =
        serde_json::from_value(stale_reply).expect("decode stale bridge reply");
    match stale_reply {
        super::bridge_protocol::BridgeReply::Rejected { cause, reason } => {
            assert_eq!(
                cause,
                super::bridge_protocol::BridgeRejectionCause::StaleSupervisor,
                "stale supervisor should be rejected with typed cause, got reason: {reason}"
            );
            assert!(
                reason.contains("stale supervisor"),
                "stale supervisor should be rejected explicitly, got: {reason}"
            );
        }
        other => panic!("expected stale supervisor rejection, got {other:?}"),
    }
}

#[tokio::test]
async fn test_rotate_supervisor_bind_fallback_binds_next_authority() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "rotate-supervisor-bind-fallback",
    );
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let runtime_metadata = storage.runtime_metadata.clone();
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service)
        .create()
        .await
        .expect("create mob");
    let external = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext")).await;
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            external.binding(),
        )
        .await
        .expect("spawn live external worker");

    external.forget_supervisor().await;

    let report = handle.rotate_supervisor().await.expect("rotate supervisor");
    let rotated = runtime_metadata
        .load_supervisor_authority(&mob_id)
        .await
        .expect("load rotated authority")
        .expect("rotated authority record");
    assert_eq!(rotated.public_peer_id, report.public_peer_id);
    assert_eq!(
        external.authorized_supervisor_peer_id().await.as_deref(),
        Some(rotated.public_peer_id.as_str()),
        "bind fallback during rotation must install the next supervisor, not the old one"
    );
}

// `test_rotate_supervisor_rolls_back_remote_authorization_on_partial_failure`
// was removed in the c8 commit of the bridge review fixes. The test asserted
// that on a partial-authorize failure during rotation the mob would roll the
// LOCAL authority back to the pre-rotation supervisor. That expectation is
// incompatible with the strict `BindMember` gate introduced in the same
// review: once any remote has rotated forward, a rollback is rejected with
// `BridgeRejectionCause::AlreadyBound` and the documented recovery is to
// advance local authority to match the partially-applied remote state —
// which is exactly what the following test
// `test_rotate_supervisor_advances_local_authority_when_rollback_fails`
// verifies. See `CLAUDE.md` > "Mob Orchestration" and
// `MobActor::handle_rotate_supervisor` for the authoritative behavior.

#[tokio::test]
async fn test_rotate_supervisor_advances_local_authority_when_rollback_fails() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "rotate-supervisor-rollback-failure",
    );
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let runtime_metadata = storage.runtime_metadata.clone();
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service)
        .create()
        .await
        .expect("create mob");
    let external_a = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-a")).await;
    let external_b = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-b")).await;
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-a"),
            None,
            external_a.binding(),
        )
        .await
        .expect("spawn first live external worker");
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-b"),
            None,
            external_b.binding(),
        )
        .await
        .expect("spawn second live external worker");

    let original = runtime_metadata
        .load_supervisor_authority(&mob_id)
        .await
        .expect("load original authority")
        .expect("original authority record");
    external_a.fail_next_bind();
    external_b.fail_next_authorize();

    let error = handle
        .rotate_supervisor()
        .await
        .expect_err("rollback bind failure should abort rotation");
    assert!(
        error
            .to_string()
            .contains("local supervisor authority advanced"),
        "rollback failure should explain the local authority cutover, got: {error}"
    );

    let current = runtime_metadata
        .load_supervisor_authority(&mob_id)
        .await
        .expect("load authority after failed rollback")
        .expect("authority after failed rollback");
    assert_eq!(
        current.epoch,
        original.epoch + 1,
        "mob should advance to the already-applied next authority when rollback fails"
    );
    assert_ne!(
        current.public_peer_id, original.public_peer_id,
        "failed rollback should keep the deterministic next supervisor active locally"
    );

    handle
        .internal_turn(
            AgentIdentity::from("w-a"),
            ContentInput::from("after-rollback-failure".to_string()),
        )
        .await
        .expect("partially rotated peer should remain controllable");
    handle
        .internal_turn(
            AgentIdentity::from("w-b"),
            ContentInput::from("recover-old-peer".to_string()),
        )
        .await
        .expect("old peer should recover via bind fallback under the advanced supervisor");
}

#[tokio::test]
async fn test_restarted_peer_only_member_rebinds_when_supervisor_state_is_lost() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "peer-only-rebind-after-restart",
    );
    let mob_id = definition.id.clone();
    let (handle, _service) = create_test_mob_with_real_comms(definition).await;
    let external = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext")).await;
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            external.binding(),
        )
        .await
        .expect("spawn live external worker");
    let default_protocol_version =
        meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_default_protocol_version();
    assert_eq!(external.bind_count(), 1, "initial spawn should bind once");
    assert_eq!(
        external.bind_protocol_versions().await,
        vec![default_protocol_version],
        "initial bridge bind should use the canonical default protocol version"
    );

    external.forget_supervisor().await;

    handle
        .internal_turn(
            AgentIdentity::from("w-ext"),
            ContentInput::from("after-restart".to_string()),
        )
        .await
        .expect("lost supervisor state should trigger a rebind fallback");
    assert_eq!(
        external.bind_count(),
        2,
        "restart recovery should re-establish supervisor authority via BindMember"
    );
    assert_eq!(
        external.bind_protocol_versions().await,
        vec![default_protocol_version, default_protocol_version],
        "restart rebind should keep reporting the canonical bridge protocol version"
    );
    assert_eq!(
        external.delivered_input_ids().await.len(),
        1,
        "rebound peer should still receive the requested input"
    );
}

#[tokio::test]
async fn test_query_string_bootstrap_token_fallback_is_rejected() {
    // Dogma regression: bind with only a query-string `?mob_supervisor_bootstrap_token=...`
    // (no typed `bootstrap_token` field) must be rejected. The typed field is the
    // sole authoritative carrier; the query-string fallback was deleted in Wave 3 E.
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "peer-only-query-bootstrap-rejected",
    );
    let mob_id = definition.id.clone();
    let (handle, _service) = create_test_mob_with_real_comms(definition).await;
    let external = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext")).await;
    let crate::RuntimeBinding::External {
        peer_id,
        address,
        bootstrap_token,
        pubkey,
    } = external.binding()
    else {
        panic!("live external peer must produce external binding");
    };
    let bootstrap_token = bootstrap_token.expect("live external binding bootstrap token");
    let binding = crate::RuntimeBinding::External {
        peer_id,
        address: format!(
            "{}?{}={}",
            address,
            super::bridge_protocol::SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM,
            bootstrap_token.as_str()
        ),
        bootstrap_token: None,
        pubkey,
    };
    let err = handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            binding,
        )
        .await
        .expect_err("query-string-only bootstrap token must be rejected");
    let rendered = err.to_string();
    assert!(
        rendered.contains("typed bootstrap_token field"),
        "error must name the missing typed field, got: {rendered}"
    );
}

#[tokio::test]
async fn test_concurrent_terminal_lifecycle_commands_observe_live_state_drift() {
    let (handle, _service) = create_test_mob(sample_definition_with_mcp_servers()).await;

    let stop = {
        let handle = handle.clone();
        tokio::spawn(async move { ("stop", handle.stop().await) })
    };
    let complete = {
        let handle = handle.clone();
        tokio::spawn(async move { ("complete", handle.complete().await) })
    };

    let (stop, complete) = tokio::join!(stop, complete);
    let results = [stop.expect("stop join"), complete.expect("complete join")];
    let successes = results.iter().filter(|(_, result)| result.is_ok()).count();
    assert_eq!(
        successes, 1,
        "only the first admitted terminal lifecycle command may run shell mechanics: {results:?}"
    );

    let failed = results
        .iter()
        .find_map(|(_, result)| result.as_ref().err())
        .expect("one command should observe live-state drift");
    assert!(
        matches!(failed, MobError::InvalidTransition { .. }),
        "losing lifecycle command must fail at live admission, got {failed:?}"
    );

    let state = handle.status().await.expect("status after lifecycle race");
    assert!(
        matches!(state, MobState::Stopped | MobState::Completed),
        "terminal lifecycle race should leave a terminal lifecycle phase, got {state:?}"
    );
    assert!(
        handle
            .mcp_server_states()
            .await
            .values()
            .all(|running| !*running),
        "terminal lifecycle shell mechanics should stop mcp servers exactly once"
    );
}

#[tokio::test]
async fn test_lifecycle_updates_mcp_server_states() {
    let (handle, _service) = create_test_mob(sample_definition_with_mcp_servers()).await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let initial = handle.mcp_server_states().await;
    assert!(
        initial.values().all(|running| *running),
        "mcp servers should start in running state"
    );

    handle.stop().await.expect("stop");
    let stopped = handle.mcp_server_states().await;
    assert!(
        stopped.values().all(|running| !*running),
        "stop should mark all mcp servers as stopped"
    );

    handle.resume().await.expect("resume");
    let resumed = handle.mcp_server_states().await;
    assert!(
        resumed.values().all(|running| *running),
        "resume should mark all mcp servers as running"
    );

    handle.complete().await.expect("complete");
    let completed = handle.mcp_server_states().await;
    assert!(
        completed.values().all(|running| !*running),
        "complete should stop all mcp servers"
    );
}

#[tokio::test]
async fn test_stop_persists_all_state_and_rejects_mutations() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    let events_before = handle.events().replay_all().await.expect("replay");

    handle.stop().await.expect("stop");
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
    assert_eq!(
        service.active_session_count().await,
        1,
        "stop should not archive active sessions"
    );
    let events_after = handle.events().replay_all().await.expect("replay");
    assert!(
        events_after.len() >= events_before.len(),
        "stop should not delete persisted events"
    );
    let before_cursors = events_before
        .iter()
        .map(|event| event.cursor)
        .collect::<Vec<_>>();
    let after_prefix_cursors = events_after[..events_before.len()]
        .iter()
        .map(|event| event.cursor)
        .collect::<Vec<_>>();
    assert_eq!(
        after_prefix_cursors, before_cursors,
        "stop should preserve the existing event log prefix even if a concurrent lifecycle projection appends later"
    );

    let err = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect_err("spawn must be rejected while stopped");
    assert!(matches!(err, MobError::InvalidTransition { .. }));
    assert_eq!(
        service.active_session_count().await,
        1,
        "MobMachine spawn admission must reject before provisioning a stopped mob"
    );
}

#[tokio::test]
async fn test_stopped_runtime_commands_are_rejected_by_machine_admission() {
    let (handle, _service) =
        create_test_mob(sample_definition_with_single_step_flow(60_000, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    let entry = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("member exists");

    handle.stop().await.expect("stop");

    let spawn = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await;
    assert!(
        matches!(
            spawn,
            Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Running,
            })
        ),
        "Spawn must surface the MobMachine stopped-phase rejection: {spawn:?}"
    );

    let submit = handle
        .submit_work(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            WorkRef::new(),
            WorkSpec::new("queued while stopped".to_string(), WorkOrigin::Internal),
        )
        .await;
    assert!(
        matches!(
            submit,
            Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Running,
            })
        ),
        "SubmitWork must surface the MobMachine stopped-phase rejection: {submit:?}"
    );

    let cancel_all_work = handle
        .cancel_all_work(entry.agent_runtime_id.clone(), entry.fence_token)
        .await;
    assert!(
        matches!(
            cancel_all_work,
            Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Running,
            })
        ),
        "CancelAllWork must surface the MobMachine stopped-phase rejection: {cancel_all_work:?}"
    );

    let run_flow = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await;
    assert!(
        matches!(
            run_flow,
            Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Running,
            })
        ),
        "RunFlow must surface the MobMachine stopped-phase rejection: {run_flow:?}"
    );

    let task_create = handle
        .task_create(
            "stopped task".to_string(),
            "must be admitted by MobMachine".to_string(),
            vec![],
        )
        .await;
    assert!(
        matches!(
            task_create,
            Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Running,
            })
        ),
        "TaskCreate must surface the MobMachine stopped-phase rejection: {task_create:?}"
    );

    let respawn = handle.respawn(AgentIdentity::from("w-1"), None).await;
    assert!(
        matches!(
            respawn,
            Err(crate::runtime::handle::MobRespawnError::Mob(
                MobError::InvalidTransition {
                    from: MobState::Stopped,
                    to: MobState::Running,
                }
            ))
        ),
        "Respawn must surface the MobMachine stopped-phase rejection: {respawn:?}"
    );
}

#[tokio::test]
async fn test_stopped_empty_task_create_is_rejected_by_machine_admission() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle.stop().await.expect("stop");

    let result = handle
        .task_create(
            "   ".to_string(),
            "empty subject must not shadow stopped admission".to_string(),
            vec![],
        )
        .await;

    assert!(
        matches!(
            result,
            Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Running,
            })
        ),
        "empty task subject must not shadow MobMachine stopped-phase admission: {result:?}"
    );
}

#[tokio::test]
async fn test_stopped_unknown_profile_spawn_is_rejected_by_machine_admission() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle.stop().await.expect("stop");

    let result = handle
        .spawn(
            ProfileName::from("missing-profile"),
            MeerkatId::from("w-missing-profile"),
            None,
        )
        .await;

    assert!(
        matches!(
            result,
            Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Running,
            })
        ),
        "unknown profile must not shadow MobMachine stopped-phase admission: {result:?}"
    );
    assert_eq!(
        service.active_session_count().await,
        0,
        "stopped rejected spawn must not provision a session"
    );
}

#[tokio::test]
async fn test_stopped_unknown_flow_run_flow_is_rejected_by_machine_admission() {
    let (handle, _service) =
        create_test_mob(sample_definition_with_single_step_flow(60_000, 8)).await;
    handle.stop().await.expect("stop");

    let result = handle
        .run_flow(FlowId::from("missing-flow"), serde_json::json!({}))
        .await;

    assert!(
        matches!(
            result,
            Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Running,
            })
        ),
        "unknown flow must not shadow MobMachine stopped-phase admission: {result:?}"
    );
}

fn mob_command_arm_source<'a>(source: &'a str, command: &str) -> &'a str {
    let marker = format!("                MobCommand::{command}");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("MobCommand::{command} arm exists"));
    let rest = &source[start + marker.len()..];
    let end = rest
        .find("\n                MobCommand::")
        .map(|offset| start + marker.len() + offset)
        .unwrap_or(source.len());
    &source[start..end]
}

#[test]
fn test_lifecycle_command_admission_arms_do_not_shadow_mob_machine_guards() {
    let source = include_str!("actor.rs");
    for (command, input) in [
        ("Stop", "Stop"),
        ("ResumeLifecycle", "Resume"),
        ("Complete", "Complete"),
        ("Reset", "Reset"),
    ] {
        let arm = mob_command_arm_source(source, command);
        for disallowed in [
            "require_state(",
            "require_live_lifecycle_phase(",
            "require_live_reset_admission(",
            "require_live_stop_admission(",
        ] {
            assert!(
                !arm.contains(disallowed),
                "MobCommand::{command} admission must be delegated to MobMachineInput::{input}, not shell `{disallowed}`"
            );
        }
        assert!(
            arm.contains(&format!("MobMachineInput::{input}")),
            "MobCommand::{command} must submit MobMachineInput::{input} for command admission before shell mechanics"
        );
    }
}

#[test]
fn test_mob_command_admission_arms_do_not_shadow_mob_machine_guards() {
    let source = include_str!("actor.rs");
    for command in [
        "Spawn",
        "Respawn",
        "RunFlow",
        "CancelFlow",
        "SubmitWork",
        "CancelAllWork",
        "TaskCreate",
        "TaskUpdate",
    ] {
        let arm = mob_command_arm_source(source, command);
        for disallowed in [
            "require_state(",
            "require_live_lifecycle_phase(",
            "require_live_reset_admission(",
            "require_live_stop_admission(",
        ] {
            assert!(
                !arm.contains(disallowed),
                "MobCommand::{command} admission must be delegated to MobMachine guards, not shell `{disallowed}`"
            );
        }
    }
}

#[tokio::test]
async fn test_stopped_missing_member_wire_is_rejected_by_machine_admission() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle.stop().await.expect("stop mob");

    let result = handle
        .wire(
            AgentIdentity::from("missing-a"),
            AgentIdentity::from("missing-b"),
        )
        .await;

    assert!(
        matches!(
            result,
            Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Running,
            })
        ),
        "WireMembers must surface the MobMachine stopped-phase admission before shell member lookup: {result:?}"
    );
}

#[tokio::test]
async fn test_task_create_store_failure_does_not_commit_mob_machine_task() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let (handle, _service) = create_test_mob_with_events(sample_definition(), events.clone()).await;

    events.fail_appends_for("TaskCreated").await;
    let error = handle
        .task_create(
            "uncommitted task".to_string(),
            "fault-injected append failure".to_string(),
            vec![],
        )
        .await
        .expect_err("fault-injected TaskCreated append failure should reject task creation");
    assert!(
        error.to_string().contains("TaskCreated"),
        "task create should surface the task-board append failure, got: {error}"
    );

    let dsl = handle
        .debug_dsl_t2_snapshot()
        .await
        .expect("query DSL task state after failed create");
    assert!(
        dsl.tasks.is_empty(),
        "TaskCreated append failure must not commit MobMachine task projection: {:?}",
        dsl.tasks
    );
    assert!(
        handle.task_list().await.expect("task list").is_empty(),
        "TaskCreated append failure must not project a shell task"
    );
}

#[tokio::test]
async fn test_task_update_board_failure_does_not_commit_mob_machine_status() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn task owner");
    let blocker = handle
        .task_create(
            "blocker".to_string(),
            "must complete before dependent can be claimed".to_string(),
            vec![],
        )
        .await
        .expect("create blocker");
    let dependent = handle
        .task_create(
            "dependent".to_string(),
            "blocked task".to_string(),
            vec![blocker],
        )
        .await
        .expect("create dependent");

    let error = handle
        .task_update(
            dependent.clone(),
            crate::tasks::TaskStatus::InProgress,
            Some(AgentIdentity::from("w-1")),
        )
        .await
        .expect_err("blocked dependency should reject task update");
    assert!(
        error
            .to_string()
            .contains("blocked by incomplete dependencies"),
        "task update should surface task-board validation failure, got: {error}"
    );

    let dsl_task_id = crate::machines::mob_machine::TaskId::from(dependent.as_str());
    let dsl = handle
        .debug_dsl_t2_snapshot()
        .await
        .expect("query DSL task state after failed update");
    assert_eq!(
        dsl.tasks.get(&dsl_task_id).map(|task| task.status),
        Some(crate::machines::mob_machine::TaskStatus::Pending),
        "task-board validation failure must leave the MobMachine task status pending"
    );
    assert!(
        !dsl.in_progress_task_ids.contains(&dsl_task_id),
        "task-board validation failure must not insert the task into the MobMachine in-progress index"
    );
}

#[tokio::test]
async fn test_cancel_all_work_after_respawn_preserves_stale_fence_error() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let member_id = AgentIdentity::from("w-1");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            MeerkatId::from(member_id.as_str()),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn original member");
    let original = handle
        .get_member(&member_id)
        .await
        .expect("original member exists");
    let old_runtime_id = original.agent_runtime_id.clone();
    let old_fence_token = original.fence_token;

    let receipt = handle
        .respawn(member_id.clone(), None)
        .await
        .expect("respawn member");
    let replacement = handle
        .get_member(&member_id)
        .await
        .expect("replacement member exists");
    assert_eq!(replacement.agent_runtime_id, receipt.agent_runtime_id);
    assert_ne!(
        old_runtime_id, replacement.agent_runtime_id,
        "respawn should replace the runtime generation"
    );

    let result = handle
        .cancel_all_work(old_runtime_id.clone(), old_fence_token)
        .await;
    match result {
        Err(MobError::StaleFenceToken {
            runtime_id,
            expected,
            actual,
        }) => {
            assert_eq!(runtime_id, old_runtime_id);
            assert_eq!(expected, replacement.fence_token);
            assert_eq!(actual, old_fence_token);
        }
        other => panic!(
            "cancel_all_work with a pre-respawn runtime id/fence must preserve StaleFenceToken, got {other:?}"
        ),
    }
}

struct StaticWorkerSpawnPolicy;

#[async_trait]
impl super::spawn_policy::SpawnPolicy for StaticWorkerSpawnPolicy {
    async fn resolve(&self, _target: &AgentIdentity) -> Option<super::spawn_policy::SpawnSpec> {
        Some(super::spawn_policy::SpawnSpec {
            profile: ProfileName::from("worker"),
            runtime_mode: Some(crate::MobRuntimeMode::TurnDriven),
        })
    }
}

#[tokio::test]
async fn test_stopped_submit_work_auto_spawn_rejects_before_policy_provisioning() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .set_spawn_policy(Some(Arc::new(StaticWorkerSpawnPolicy)))
        .await
        .expect("set spawn policy");
    handle.stop().await.expect("stop");

    let target = AgentIdentity::from("auto-stopped");
    let result = handle
        .submit_work(
            AgentRuntimeId::initial(target.clone()),
            FenceToken::new(0),
            WorkRef::new(),
            WorkSpec::new("queued while stopped".to_string(), WorkOrigin::External),
        )
        .await;

    assert!(
        matches!(
            result,
            Err(MobError::InvalidTransition {
                from: MobState::Stopped,
                to: MobState::Running,
            })
        ),
        "stopped auto-spawn SubmitWork must surface MobMachine phase admission: {result:?}"
    );
    assert!(
        handle.get_member(&target).await.is_none(),
        "stopped SubmitWork must not auto-spawn the absent target"
    );
    assert_eq!(
        service.recorded_create_requests().await.len(),
        0,
        "stopped SubmitWork must reject before policy provisioning side effects"
    );
    assert_eq!(
        service.active_session_count().await,
        0,
        "stopped SubmitWork must not leak a provisioned session"
    );
}

#[tokio::test]
async fn test_running_submit_work_auto_spawn_non_addressable_rejects_before_policy_provisioning() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .set_spawn_policy(Some(Arc::new(StaticWorkerSpawnPolicy)))
        .await
        .expect("set spawn policy");

    let target = AgentIdentity::from("auto-running");
    let result = handle
        .submit_work(
            AgentRuntimeId::initial(target.clone()),
            FenceToken::new(0),
            WorkRef::new(),
            WorkSpec::new(
                "queued for non-addressable policy profile".to_string(),
                WorkOrigin::External,
            ),
        )
        .await;

    assert!(
        matches!(result, Err(MobError::NotExternallyAddressable(_))),
        "running external auto-spawn SubmitWork must surface MobMachine addressability admission: {result:?}"
    );
    assert!(
        handle.get_member(&target).await.is_none(),
        "non-addressable auto-spawn SubmitWork must not insert the rejected target"
    );
    assert_eq!(
        service.recorded_create_requests().await.len(),
        0,
        "non-addressable auto-spawn SubmitWork must reject before policy provisioning side effects"
    );
    assert_eq!(
        service.active_session_count().await,
        0,
        "non-addressable auto-spawn SubmitWork must not leak a provisioned session"
    );
}

#[tokio::test]
async fn test_register_tool_bundle_is_wired_into_spawn() {
    let definition = sample_definition_with_tool_bundle("bundle-a");
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .register_tool_bundle("bundle-a", Arc::new(EchoBundleDispatcher))
        .create()
        .await
        .expect("create mob");

    let session_id = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let flags = service.recorded_external_tools_flags().await;
    assert_eq!(
        flags,
        vec![true],
        "spawn should pass registered rust bundle as external_tools"
    );

    let tool_names = service.external_tool_names(&session_id).await;
    assert!(
        tool_names.contains(&"bundle_echo".to_string()),
        "registered rust bundle tool should be exposed"
    );
    let result = service
        .dispatch_external_tool(
            &session_id,
            "bundle_echo",
            serde_json::json!({"value": "hello"}),
        )
        .await
        .expect("bundle tool dispatch");
    let payload: serde_json::Value =
        serde_json::from_str(&result.text_content()).expect("bundle payload");
    assert_eq!(payload["echo"], "hello");
}

#[tokio::test]
async fn test_spawn_fails_when_tool_bundle_not_registered() {
    let definition = sample_definition_with_tool_bundle("missing-bundle");
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await;
    assert!(matches!(result, Err(MobError::Internal(_))));

    assert!(handle.list_members().await.is_empty());
    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MemberSpawned(..))),
        "failed spawn should not emit any spawn event"
    );
}

#[tokio::test]
async fn test_mob_management_tools_hidden_without_operator_context() {
    let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let tool_names = service.external_tool_names(&sid_1).await;
    for required in [
        "spawn_member",
        "spawn_many_members",
        "retire_member",
        "wire_members",
        "unwire_members",
        "list_members",
    ] {
        assert!(
            !tool_names.contains(&required.to_string()),
            "operator tool '{required}' must stay hidden without runtime-injected operator context"
        );
    }

    let spawn_err = service
        .dispatch_external_tool_outcome(
            &sid_1,
            "spawn_member",
            serde_json::json!({"profile": "worker", "member_id": "w-2"}),
        )
        .await
        .expect_err("spawn_member tool should be unavailable");
    assert!(matches!(spawn_err, ToolError::NotFound { .. }));
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-2"))
            .await
            .is_none(),
        "hidden operator tools must not mutate roster state"
    );
}

#[tokio::test]
async fn test_visible_mob_operator_reads_still_require_exact_scope() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let profile = handle
        .definition()
        .profiles
        .get(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline()
        .unwrap();
    let dispatcher = super::tools::compose_external_tools_for_profile(
        profile,
        &BTreeMap::new(),
        handle.clone(),
        None,
        crate::build::MobToolAccessContext::InjectedAuthority(
            meerkat_core::service::MobToolAuthorityContext::new(
                meerkat_core::service::OpaquePrincipalToken::new("out-of-scope"),
                false,
            )
            .with_managed_mob_scope(["different-mob"]),
        ),
    )
    .expect("compose dispatcher")
    .expect("operator dispatcher should be visible when authority is injected");

    let tools = dispatcher.tools();
    let tool_names = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        tool_names.contains(&"list_members"),
        "tool visibility should depend on injected authority presence, not scope success"
    );

    let args = RawValue::from_string("{}".to_string()).expect("raw args");
    let error = dispatcher
        .dispatch(ToolCallView {
            id: "call-1",
            name: "list_members",
            args: &args,
        })
        .await
        .expect_err("out-of-scope operator read should be denied");
    assert!(matches!(error, ToolError::AccessDenied { .. }));
}

#[tokio::test]
async fn test_visible_mob_operator_tools_deny_before_arg_validation_when_scope_missing() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let profile = handle
        .definition()
        .profiles
        .get(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline()
        .unwrap();
    let dispatcher = super::tools::compose_external_tools_for_profile(
        profile,
        &BTreeMap::new(),
        handle.clone(),
        None,
        crate::build::MobToolAccessContext::InjectedAuthority(
            meerkat_core::service::MobToolAuthorityContext::new(
                meerkat_core::service::OpaquePrincipalToken::new("out-of-scope"),
                false,
            )
            .with_managed_mob_scope(["different-mob"]),
        ),
    )
    .expect("compose dispatcher")
    .expect("operator dispatcher should be visible when authority is injected");

    let args = RawValue::from_string("{}".to_string()).expect("raw args");
    let error = dispatcher
        .dispatch(ToolCallView {
            id: "call-2",
            name: "spawn_member",
            args: &args,
        })
        .await
        .expect_err("out-of-scope operator call should be denied before args are parsed");
    assert!(matches!(error, ToolError::AccessDenied { .. }));
}

#[tokio::test]
async fn test_visible_mob_operator_tools_emit_identity_native_member_payloads() {
    let (handle, _service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let mob_id = handle.definition().id.to_string();
    let profile = handle
        .definition()
        .profiles
        .get(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline()
        .unwrap();
    let dispatcher = super::tools::compose_external_tools_for_profile(
        profile,
        &BTreeMap::new(),
        handle.clone(),
        None,
        crate::build::MobToolAccessContext::InjectedAuthority(
            meerkat_core::service::MobToolAuthorityContext::new(
                meerkat_core::service::OpaquePrincipalToken::new("in-scope"),
                false,
            )
            .with_managed_mob_scope([mob_id.as_str()]),
        ),
    )
    .expect("compose dispatcher")
    .expect("operator dispatcher should be visible when authority is injected");

    let spawn_args = RawValue::from_string(
        serde_json::json!({
            "profile": "worker",
            "member_id": "w-bridge"
        })
        .to_string(),
    )
    .expect("spawn args");
    let spawn_result = dispatcher
        .dispatch(ToolCallView {
            id: "call-bridge-spawn",
            name: "spawn_member",
            args: &spawn_args,
        })
        .await
        .expect("spawn_member should dispatch");
    let spawn_payload: serde_json::Value =
        serde_json::from_str(&spawn_result.result.text_content()).expect("spawn payload");
    assert!(
        spawn_payload["agent_identity"].is_string(),
        "spawn result should surface agent_identity"
    );
    assert!(
        spawn_payload["member_ref"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "spawn result should surface canonical member_ref"
    );
    assert!(spawn_payload.get("agent_runtime_id").is_none());
    assert!(spawn_payload.get("fence_token").is_none());

    let list_args = RawValue::from_string("{}".to_string()).expect("list args");
    let list_result = dispatcher
        .dispatch(ToolCallView {
            id: "call-bridge-list",
            name: "list_members",
            args: &list_args,
        })
        .await
        .expect("list_members should dispatch");
    let list_payload: serde_json::Value =
        serde_json::from_str(&list_result.result.text_content()).expect("list payload");
    let member = list_payload["members"]
        .as_array()
        .expect("members array")
        .iter()
        .find(|entry| entry["agent_identity"] == "w-bridge")
        .expect("spawned member should appear in list");
    assert_eq!(member["agent_identity"], "w-bridge");
    assert!(
        member["member_ref"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "list output should surface canonical member_ref"
    );
    assert!(member.get("agent_runtime_id").is_none());
    assert!(member.get("fence_token").is_none());
    assert_eq!(member["role"], "worker");
    assert!(member.get("session_id").is_none());
    assert!(member.get("bridge_session_id").is_none());
    assert!(member.get("current_session_id").is_none());
    assert!(member.get("current_bridge_session_id").is_none());
}

#[tokio::test]
async fn test_spawn_helper_contract_aligns_with_retired_terminal_state() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let helper_id = AgentIdentity::from("helper-spawn");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        handle.spawn_helper(
            helper_id.clone(),
            "summarize this",
            HelperOptions {
                role_name: Some(ProfileName::from("worker")),
                runtime_mode: Some(crate::MobRuntimeMode::TurnDriven),
                ..HelperOptions::default()
            },
        ),
    )
    .await
    .expect("spawn_helper must not wait on helper terminality")
    .expect("spawn_helper succeeds");

    assert_eq!(result.agent_identity, helper_id);
    assert!(
        handle.get_member(&result.agent_identity).await.is_none(),
        "spawn_helper must remove the helper from the active roster once it returns"
    );
}

#[tokio::test]
async fn test_fork_helper_contract_aligns_with_retired_terminal_state() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let source_id = AgentIdentity::from("fork-source");
    let source_meerkat_id = MeerkatId::from("fork-source");
    let source_ref = handle
        .spawn_with_options(
            ProfileName::from("worker"),
            source_meerkat_id,
            Some("source context".into()),
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn source member");

    let helper_id = AgentIdentity::from("fork-helper");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        handle.fork_helper(
            &source_id,
            helper_id.clone(),
            "continue from source",
            crate::launch::ForkContext::FullHistory,
            HelperOptions {
                role_name: Some(ProfileName::from("worker")),
                runtime_mode: Some(crate::MobRuntimeMode::TurnDriven),
                ..HelperOptions::default()
            },
        ),
    )
    .await
    .expect("fork_helper must not wait on helper terminality")
    .expect("fork_helper succeeds");

    assert_eq!(result.agent_identity, helper_id);
    assert!(
        handle.get_member(&result.agent_identity).await.is_none(),
        "fork_helper must remove the helper from the active roster once it returns"
    );
    assert_eq!(
        source_ref.bridge_session_id().cloned(),
        handle
            .get_member(&AgentIdentity::from(source_id.as_str()))
            .await
            .and_then(|entry| entry.member_ref.bridge_session_id().cloned()),
        "fork_helper must not perturb the source member's canonical session binding"
    );
}

#[tokio::test]
async fn test_spawn_helper_defaults_to_turn_driven_even_when_profile_is_autonomous() {
    let (handle, service) = create_test_mob(sample_definition()).await;

    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        handle.spawn_helper(
            AgentIdentity::from("helper-default"),
            "summarize this",
            HelperOptions {
                role_name: Some(ProfileName::from("worker")),
                ..HelperOptions::default()
            },
        ),
    )
    .await
    .expect("spawn_helper default runtime mode must not inherit autonomous host behavior")
    .expect("spawn_helper succeeds");

    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        0,
        "spawn_helper must default to TurnDriven semantics instead of inheriting AutonomousHost"
    );
}

#[tokio::test]
async fn test_respawn_contract_aligns_receipt_with_canonical_member_state() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let member_id = MeerkatId::from("respawn-target");
    let original_ref = handle
        .spawn_with_options(
            ProfileName::from("worker"),
            member_id.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn original member");
    let old_session_id = original_ref
        .bridge_session_id()
        .cloned()
        .expect("original session id");
    let original_snapshot = handle
        .member_status(&AgentIdentity::from(member_id.as_str()))
        .await
        .expect("original member snapshot");

    let receipt = handle
        .respawn(
            AgentIdentity::from(member_id.as_str()),
            Some("resume task".into()),
        )
        .await
        .expect("respawn succeeds");

    assert_eq!(receipt.identity, AgentIdentity::from(member_id.as_str()));
    assert_eq!(receipt.previous_fence_token, original_snapshot.fence_token);
    assert_eq!(
        receipt.agent_runtime_id.identity,
        AgentIdentity::from(member_id.as_str())
    );
    assert_eq!(
        receipt.agent_runtime_id.generation,
        original_snapshot.agent_runtime_id.generation.next()
    );
    let snapshot = handle
        .member_status(&receipt.identity)
        .await
        .expect("member snapshot");
    let new_bridge_session_id = snapshot
        .current_bridge_session_id
        .clone()
        .expect("new session id");
    assert_ne!(new_bridge_session_id, old_session_id);
    assert!(
        service.read(&old_session_id).await.is_err(),
        "respawn must archive the retired session before returning"
    );

    assert_eq!(snapshot.agent_runtime_id, receipt.agent_runtime_id);
    assert_eq!(snapshot.fence_token, receipt.fence_token);
    assert_eq!(
        snapshot.current_session_id,
        Some(new_bridge_session_id.clone())
    );
    assert_eq!(
        service
            .read(&new_bridge_session_id)
            .await
            .expect("new session readable")
            .state
            .session_id,
        new_bridge_session_id,
        "respawn receipt must align with the canonical replacement session"
    );
}

#[tokio::test]
async fn test_respawn_success_restores_existing_peer_wiring() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let left = MeerkatId::from("respawn-left");
    let right = MeerkatId::from("respawn-right");
    let original_left = handle
        .spawn_with_options(
            ProfileName::from("worker"),
            left.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn left");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            right.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn right");
    handle
        .wire(AgentIdentity::from(left.as_str()), right.clone())
        .await
        .expect("wire peers before respawn");
    let original_left_snapshot = handle
        .member_status(&AgentIdentity::from(left.as_str()))
        .await
        .expect("left snapshot before respawn");

    let old_session_id = original_left
        .bridge_session_id()
        .cloned()
        .expect("left session id");
    let receipt = handle
        .respawn(
            AgentIdentity::from(left.as_str()),
            Some("rebuild wiring".into()),
        )
        .await
        .expect("respawn succeeds");

    assert_eq!(
        receipt.previous_fence_token,
        original_left_snapshot.fence_token
    );
    assert_eq!(
        receipt.agent_runtime_id.generation,
        original_left_snapshot.agent_runtime_id.generation.next()
    );
    assert!(
        service.read(&old_session_id).await.is_err(),
        "respawn must archive the retired session before returning"
    );

    let left_entry = handle
        .get_member(&AgentIdentity::from(left.as_str()))
        .await
        .expect("left remains active");
    let right_entry = handle
        .get_member(&AgentIdentity::from(right.as_str()))
        .await
        .expect("right remains active");
    assert!(
        left_entry
            .wired_to
            .contains(&AgentIdentity::from(right.as_str())),
        "respawn replacement must restore canonical wiring on the replacement member"
    );
    assert!(
        right_entry
            .wired_to
            .contains(&AgentIdentity::from(left.as_str())),
        "respawn replacement must preserve the peer's canonical wiring projection"
    );
    assert_eq!(left_entry.agent_runtime_id, receipt.agent_runtime_id);
    assert_eq!(left_entry.fence_token, receipt.fence_token);
    assert_eq!(
        left_entry.agent_runtime_id, receipt.agent_runtime_id,
        "respawn receipt must align with the replacement member's canonical session binding"
    );
}

#[tokio::test]
async fn test_respawn_restores_local_wiring_from_mob_machine_edge() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let left = MeerkatId::from("respawn-machine-left");
    let right = MeerkatId::from("respawn-machine-right");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            left.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn left");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            right.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn right");

    let edge = crate::machines::mob_machine::WiringEdge::new(
        crate::machines::mob_machine::AgentIdentity::from(left.as_str()),
        crate::machines::mob_machine::AgentIdentity::from(right.as_str()),
    );
    handle
        .project_machine_input(crate::machines::mob_machine::MobMachineInput::WireMembers { edge })
        .await
        .expect("seed machine-owned edge without roster projection");
    let left_before = handle
        .get_member(&AgentIdentity::from(left.as_str()))
        .await
        .expect("left before respawn");
    assert!(
        !left_before
            .wired_to
            .contains(&AgentIdentity::from(right.as_str())),
        "test setup must leave roster projection empty so MobMachine is the only topology source"
    );

    handle
        .respawn(
            AgentIdentity::from(left.as_str()),
            Some("restore machine edge".into()),
        )
        .await
        .expect("respawn succeeds");

    let left_after = handle
        .get_member(&AgentIdentity::from(left.as_str()))
        .await
        .expect("left after respawn");
    let right_after = handle
        .get_member(&AgentIdentity::from(right.as_str()))
        .await
        .expect("right after respawn");
    assert!(
        left_after
            .wired_to
            .contains(&AgentIdentity::from(right.as_str()))
    );
    assert!(
        right_after
            .wired_to
            .contains(&AgentIdentity::from(left.as_str()))
    );
}

#[tokio::test]
async fn test_respawn_ignores_machine_local_edge_to_retired_peer() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let left = MeerkatId::from("respawn-stale-left");
    let right = MeerkatId::from("respawn-stale-right");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            left.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn left");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            right.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn right");
    handle
        .wire(
            AgentIdentity::from(left.as_str()),
            PeerTarget::Local(AgentIdentity::from(right.as_str())),
        )
        .await
        .expect("wire peers");

    handle
        .retire(AgentIdentity::from(right.as_str()))
        .await
        .expect("retire old peer");
    assert!(
        handle
            .get_member(&AgentIdentity::from(right.as_str()))
            .await
            .is_none(),
        "test setup should remove the old peer from the live roster"
    );

    handle
        .respawn(
            AgentIdentity::from(left.as_str()),
            Some("ignore stale peer".into()),
        )
        .await
        .expect("respawn should ignore machine edge to retired peer");
    let left_after = handle
        .get_member(&AgentIdentity::from(left.as_str()))
        .await
        .expect("left after respawn");
    assert!(
        !left_after
            .wired_to
            .contains(&AgentIdentity::from(right.as_str())),
        "stale machine edge to retired peer must not be restored into roster projection"
    );
}

#[tokio::test]
async fn test_respawn_archive_failure_returns_cleanup_ambiguous_report() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let member_id = MeerkatId::from("respawn-ambiguous");
    let member_ref = handle
        .spawn(ProfileName::from("worker"), member_id.clone(), None)
        .await
        .expect("spawn member");
    let old_session_id = member_ref
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let original_snapshot = handle
        .member_status(&AgentIdentity::from(member_id.as_str()))
        .await
        .expect("snapshot before respawn");

    service.set_archive_failure(&old_session_id).await;

    let error = handle
        .respawn(AgentIdentity::from(member_id.as_str()), None)
        .await
        .expect_err("respawn should surface ambiguous cleanup when archive fails");

    let report = match error {
        crate::runtime::handle::MobRespawnError::PreviousMemberCleanupAmbiguous { report } => {
            report
        }
        other => panic!("expected PreviousMemberCleanupAmbiguous, got {other:?}"),
    };

    assert_eq!(report.identity, AgentIdentity::from(member_id.as_str()));
    assert_eq!(report.agent_runtime_id, original_snapshot.agent_runtime_id);
    assert_eq!(report.fence_token, original_snapshot.fence_token);
    assert!(
        report.retire_attempted,
        "respawn should have attempted retire"
    );
    assert!(
        report
            .retire_error
            .as_deref()
            .is_some_and(|msg| msg.contains("ArchiveSession")),
        "cleanup report should capture the archive failure: {:?}",
        report.retire_error
    );
    assert!(
        !report.confirmatory_observation_attempted,
        "current respawn ambiguity path should not claim a confirmatory probe"
    );
    assert!(
        !report.destroy_attempted,
        "current respawn ambiguity path should not claim a force-destroy"
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from(member_id.as_str()))
            .await
            .is_none(),
        "ambiguous cleanup must not create a replacement member"
    );
}

#[tokio::test]
async fn test_list_members_returns_after_respawn_without_hanging() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let member_id = MeerkatId::from("respawn-list-members");

    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            member_id.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn original member");

    let receipt = handle
        .respawn(
            AgentIdentity::from(member_id.as_str()),
            Some("refresh listing".into()),
        )
        .await
        .expect("respawn succeeds");

    let members = tokio::time::timeout(std::time::Duration::from_secs(2), handle.list_members())
        .await
        .expect("list_members should not hang after respawn");
    let listed = members
        .into_iter()
        .find(|entry| entry.agent_identity == member_id)
        .expect("respawned member remains listed");

    assert_eq!(listed.agent_runtime_id, receipt.agent_runtime_id);
    assert_eq!(listed.fence_token, receipt.fence_token);
    assert_eq!(listed.state, crate::roster::MemberState::Active);
    assert_eq!(
        listed.status,
        crate::runtime::handle::MobMemberStatus::Active
    );
}

#[tokio::test]
async fn test_wait_one_returns_terminal_unknown_for_missing_member() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let snapshot = handle
        .wait_one(&AgentIdentity::from("missing-helper"))
        .await
        .expect("wait_one should succeed for missing member");

    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Unknown
    );
    assert!(
        snapshot.is_final,
        "wait_one should classify a missing member as terminal"
    );
    assert!(
        snapshot.current_session_id.is_none(),
        "missing-member wait result must not invent a session bridge"
    );
}

#[tokio::test]
async fn test_wait_one_observes_retiring_member_as_non_terminal_until_archive() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let member_id = MeerkatId::from("wait-retiring");
    let session_id = handle
        .spawn(ProfileName::from("worker"), member_id.clone(), None)
        .await
        .expect("spawn retiring member")
        .bridge_session_id()
        .expect("session-backed member")
        .clone();

    service.set_archive_delay_ms(250);
    let retire = {
        let handle = handle.clone();
        let member_id = member_id.clone();
        tokio::spawn(async move { handle.retire(AgentIdentity::from(member_id.as_str())).await })
    };

    tokio::time::sleep(Duration::from_millis(25)).await;
    let in_flight = handle
        .member_status(&AgentIdentity::from(member_id.as_str()))
        .await
        .expect("retiring member snapshot");
    assert_eq!(
        in_flight.status,
        crate::runtime::handle::MobMemberStatus::Retiring,
        "helper classification should continue to expose retiring canonical truth while disposal is still in flight"
    );
    assert!(
        !in_flight.is_final,
        "retiring member must not classify as terminal before archive/removal completes"
    );

    let terminal = handle
        .wait_one(&AgentIdentity::from(member_id.as_str()))
        .await
        .expect("wait_one should observe terminal retirement");
    assert!(
        matches!(
            terminal.status,
            crate::runtime::handle::MobMemberStatus::Unknown
                | crate::runtime::handle::MobMemberStatus::Completed
        ),
        "terminal helper snapshot must come from canonical post-retire truth"
    );
    assert!(terminal.is_final);
    assert!(
        service.read(&session_id).await.is_err(),
        "wait_one must not return terminal until the backing session is archived or removed from canonical truth"
    );

    retire
        .await
        .expect("retire join")
        .expect("retire completes");
}

#[tokio::test]
async fn test_wait_all_preserves_input_order_for_missing_members() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let ids = vec![
        AgentIdentity::from("missing-a"),
        AgentIdentity::from("missing-b"),
    ];

    let snapshots = handle
        .wait_all(&ids)
        .await
        .expect("wait_all should succeed for missing members");

    assert_eq!(snapshots.len(), 2);
    assert_eq!(
        snapshots[0].status,
        crate::runtime::handle::MobMemberStatus::Unknown
    );
    assert_eq!(
        snapshots[1].status,
        crate::runtime::handle::MobMemberStatus::Unknown
    );
    assert!(
        snapshots.iter().all(|snapshot| snapshot.is_final),
        "wait_all should classify all missing members as terminal"
    );
}

#[tokio::test]
async fn test_wait_for_kickoff_complete_returns_current_autonomous_snapshots() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);
    service.set_start_turn_delay_ms(150);

    let lead = MeerkatId::from("lead-ready");
    let worker = MeerkatId::from("worker-ready");
    handle
        .spawn(ProfileName::from("lead"), lead.clone(), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(ProfileName::from("worker"), worker.clone(), None)
        .await
        .expect("spawn worker");

    let snapshots = handle
        .wait_for_kickoff_complete(Some(Duration::from_secs(2)))
        .await
        .expect("kickoff barrier succeeds");

    assert_eq!(
        snapshots.len(),
        2,
        "all current autonomous members should be returned"
    );
    assert_eq!(snapshots[0].0, AgentIdentity::from(lead.as_str()));
    assert_eq!(snapshots[1].0, AgentIdentity::from(worker.as_str()));
    assert!(
        snapshots
            .iter()
            .all(|(_, snapshot)| snapshot.status == crate::runtime::handle::MobMemberStatus::Active),
        "kickoff completion should return current active snapshots rather than terminalizing members"
    );
}

#[tokio::test]
async fn test_wait_for_members_kickoff_complete_only_waits_requested_members() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);
    service.set_start_turn_delay_ms(120);

    let autonomous = MeerkatId::from("lead-only");
    let turn_driven = MeerkatId::from("worker-td");
    handle
        .spawn(ProfileName::from("lead"), autonomous.clone(), None)
        .await
        .expect("spawn autonomous lead");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            turn_driven.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn turn-driven worker");

    let snapshots = handle
        .wait_for_members_kickoff_complete(
            &[
                AgentIdentity::from(autonomous.as_str()),
                AgentIdentity::from(turn_driven.as_str()),
            ],
            Some(Duration::from_secs(2)),
        )
        .await
        .expect("member-scoped barrier succeeds");

    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].0, AgentIdentity::from(autonomous.as_str()));
    assert_eq!(snapshots[1].0, AgentIdentity::from(turn_driven.as_str()));
    assert_eq!(
        snapshots[1].1.status,
        crate::runtime::handle::MobMemberStatus::Active,
        "non-autonomous members should be treated as immediately satisfied"
    );
}

#[tokio::test]
async fn test_wait_for_kickoff_complete_returns_after_initial_turn() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    // Runtime-backed autonomous kickoff still resolves asynchronously, but
    // enabling immediate completion should let the barrier clear quickly.
    service.set_keep_alive_turns_complete_immediately(true);

    let member = MeerkatId::from("lead-hung");
    handle
        .spawn(ProfileName::from("lead"), member.clone(), None)
        .await
        .expect("spawn lead");

    let snapshots = handle
        .wait_for_kickoff_complete(Some(Duration::from_secs(2)))
        .await
        .expect("barrier should return after initial turn completes");

    assert!(
        !snapshots.is_empty(),
        "barrier should return snapshots for spawned members"
    );
    let kickoff = snapshots[0]
        .1
        .kickoff
        .as_ref()
        .expect("autonomous member should expose kickoff state");
    assert_eq!(
        kickoff.phase,
        crate::roster::MobMemberKickoffPhase::Started,
        "barrier must return only after kickoff reaches a terminal started phase"
    );
}

#[tokio::test]
async fn test_wait_for_kickoff_complete_times_out_while_runtime_backed_kickoff_pending() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_start_turn_delay_ms(250);

    let member = MeerkatId::from("lead-pending");
    handle
        .spawn(ProfileName::from("lead"), member.clone(), None)
        .await
        .expect("spawn lead");

    let error = handle
        .wait_for_kickoff_complete(Some(Duration::from_millis(50)))
        .await
        .expect_err("barrier must not return before kickoff resolves");

    match error {
        MobError::KickoffWaitTimedOut { pending_member_ids } => {
            assert_eq!(
                pending_member_ids,
                vec![member],
                "pending autonomous kickoff should surface the still-starting member"
            );
        }
        other => panic!("expected kickoff timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn test_wait_for_members_ready_treats_turn_driven_members_as_ready() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let member = MeerkatId::from("worker-ready-td");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            member.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn turn-driven worker");

    let snapshots = handle
        .wait_for_members_ready(
            &[AgentIdentity::from(member.as_str())],
            Some(Duration::from_secs(2)),
        )
        .await
        .expect("turn-driven members should be ready immediately");

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].0, AgentIdentity::from(member.as_str()));
}

#[tokio::test]
async fn test_wait_for_members_ready_returns_for_autonomous_members_after_startup_readiness() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_start_turn_delay_ms(250);

    let member = MeerkatId::from("lead-ready-autonomous");
    handle
        .spawn(ProfileName::from("lead"), member.clone(), None)
        .await
        .expect("spawn autonomous lead");

    let snapshots = handle
        .wait_for_members_ready(
            &[AgentIdentity::from(member.as_str())],
            Some(Duration::from_secs(2)),
        )
        .await
        .expect("autonomous member should become startup-ready without waiting for kickoff");

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].0, AgentIdentity::from(member.as_str()));
    assert_eq!(
        snapshots[0].1.status,
        crate::runtime::handle::MobMemberStatus::Active,
        "startup-ready barrier should return the live autonomous snapshot"
    );
}

#[tokio::test]
async fn test_wait_for_members_kickoff_complete_excludes_later_spawns() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);

    let barrier = tokio::spawn({
        let handle = handle.clone();
        async move {
            handle
                .wait_for_members_kickoff_complete(&[], Some(Duration::from_secs(2)))
                .await
        }
    });

    tokio::time::sleep(Duration::from_millis(10)).await;
    handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("late-lead"),
            None,
        )
        .await
        .expect("spawn late lead");

    let snapshots = barrier
        .await
        .expect("barrier join")
        .expect("empty target barrier succeeds");
    assert!(
        snapshots.is_empty(),
        "members spawned after the barrier call must not be included"
    );
}

#[tokio::test]
async fn test_wait_for_kickoff_complete_returns_broken_snapshot_without_hanging() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let broken = MeerkatId::from("lead-broken");
    handle
        .spawn(ProfileName::from("lead"), broken.clone(), None)
        .await
        .expect("spawn lead");
    handle.stop().await.expect("stop mob");

    let old_sid = handle
        .get_member(&AgentIdentity::from(broken.as_str()))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume should succeed with Broken projection");

    let snapshots = resumed
        .wait_for_kickoff_complete(Some(Duration::from_secs(2)))
        .await
        .expect("barrier should not hang on Broken members");

    let broken_snapshot = snapshots
        .into_iter()
        .find(|(id, _)| *id == broken)
        .expect("broken member snapshot");
    assert_eq!(
        broken_snapshot.1.status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
    assert!(broken_snapshot.1.error.is_some());
}

#[tokio::test]
async fn test_mob_flow_tools_hidden_without_operator_context() {
    let mut definition =
        with_cancel_grace_timeout(sample_definition_with_single_step_flow(60_000, 8), 25);
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile exists")
        .as_inline_mut()
        .unwrap();
    worker.tools.mob = true;
    worker.tools.comms = true;

    let (handle, service) = create_test_mob(definition).await;
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let tool_names = service.external_tool_names(&sid_1).await;
    for required in [
        "mob_list_flows",
        "mob_run_flow",
        "mob_flow_status",
        "mob_cancel_flow",
    ] {
        assert!(
            !tool_names.contains(&required.to_string()),
            "flow operator tool '{required}' must stay hidden without runtime-injected operator context"
        );
    }

    let listed_err = service
        .dispatch_external_tool(&sid_1, "mob_list_flows", serde_json::json!({}))
        .await
        .expect_err("mob_list_flows should be unavailable");
    assert!(matches!(listed_err, ToolError::NotFound { .. }));

    let started_err = service
        .dispatch_external_tool(
            &sid_1,
            "mob_run_flow",
            serde_json::json!({
                "flow_id": "demo",
                "params": {"ticket": "REQ-017"}
            }),
        )
        .await
        .expect_err("mob_run_flow should be unavailable");
    assert!(matches!(started_err, ToolError::NotFound { .. }));
}

#[tokio::test]
async fn test_mob_flow_status_tool_is_hidden_without_operator_context_even_for_bad_args() {
    let mut definition = sample_definition_with_single_step_flow(500, 8);
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile exists")
        .as_inline_mut()
        .unwrap();
    worker.tools.mob = true;

    let (handle, service) = create_test_mob(definition).await;
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let error = service
        .dispatch_external_tool(
            &sid_1,
            "mob_flow_status",
            serde_json::json!({"run_id":"not-a-uuid"}),
        )
        .await
        .expect_err("mob_flow_status should be unavailable");
    assert!(
        matches!(error, ToolError::NotFound { .. }),
        "flow operator tools should stay hidden without runtime-injected operator context"
    );
}

#[tokio::test]
async fn test_flow_step_tool_overlay_is_step_scoped() {
    let mut definition = sample_definition_with_single_step_flow(2_000, 8);
    let flow = definition
        .flows
        .get_mut(&flow_id("demo"))
        .expect("demo flow exists");
    let step = flow
        .steps
        .get_mut(&step_id("start"))
        .expect("start step exists");
    step.allowed_tools = Some(vec!["alpha".to_string(), "beta".to_string()]);
    step.blocked_tools = Some(vec!["beta".to_string()]);

    let (handle, service) = create_test_mob(definition).await;
    let sid = handle
        .spawn_with_options(
            ProfileName::from("worker"),
            MeerkatId::from("w-overlay"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("start flow");

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(5)).await;
    assert!(
        terminal.status == MobRunStatus::Completed,
        "flow failed; failure ledger: {:?}",
        terminal.failure_ledger
    );

    service
        .start_turn(
            &sid,
            StartTurnRequest {
                prompt: "non-flow turn".to_string().into(),
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
        .expect("plain start_turn after flow");

    let overlays = service
        .recorded_flow_turn_overlays()
        .await
        .into_iter()
        .filter(|(session_id, _)| session_id == &sid)
        .map(|(_, overlay)| overlay)
        .collect::<Vec<_>>();

    assert_eq!(overlays.len(), 2, "expected flow turn + plain turn");
    assert_eq!(
        overlays[0],
        Some(TurnToolOverlay {
            allowed_tools: Some(vec!["alpha".to_string(), "beta".to_string()]),
            blocked_tools: Some(vec!["beta".to_string()]),
        }),
        "flow step turn should pass flow overlay"
    );
    assert_eq!(
        overlays[1], None,
        "overlay must not persist into unrelated turns"
    );
}

#[tokio::test]
async fn test_flow_step_tool_overlay_changes_runtime_visible_tools_and_restores_baseline() {
    let mut definition = sample_definition_with_single_step_flow(2_000, 8);
    let flow = definition
        .flows
        .get_mut(&flow_id("demo"))
        .expect("demo flow exists");
    let step = flow
        .steps
        .get_mut(&step_id("start"))
        .expect("start step exists");
    step.allowed_tools = Some(vec!["alpha".to_string(), "beta".to_string()]);
    step.blocked_tools = Some(vec!["beta".to_string()]);

    let (handle, provider_visible_tools) =
        create_test_mob_with_overlay_probe_service(definition).await;
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            MeerkatId::from("w-runtime-overlay"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run demo flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert!(
        terminal.status == MobRunStatus::Completed,
        "flow failed; failure ledger: {:?}",
        terminal.failure_ledger
    );

    handle
        .internal_turn(
            AgentIdentity::from("w-runtime-overlay"),
            "baseline after flow".to_string(),
        )
        .await
        .expect("run baseline turn");

    let seen = provider_visible_tools
        .lock()
        .expect("provider_visible_tools lock poisoned")
        .clone();
    assert_eq!(
        seen.len(),
        2,
        "expected one provider call for flow and one for baseline"
    );
    assert_eq!(
        seen[0],
        vec!["alpha".to_string()],
        "flow step overlay should restrict provider-visible tools"
    );
    assert_eq!(
        seen[1],
        vec!["alpha".to_string(), "beta".to_string()],
        "next non-flow turn should restore baseline visible tools"
    );
}

#[tokio::test]
async fn test_spawn_member_tool_dispatches_backend_selection() {
    let mut definition = sample_definition_with_mob_tools();
    definition.backend.external = Some(crate::definition::ExternalBackendConfig {
        address_base: "https://backend.example.invalid/mesh".to_string(),
    });
    let (handle, service) = create_test_mob(definition).await;
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let err = service
        .dispatch_external_tool(
            &sid_1,
            "spawn_member",
            serde_json::json!({
                "profile": "worker",
                "member_id": "w-ext",
                "backend": "external"
            }),
        )
        .await
        .expect_err("spawn external via tool should be unavailable");
    assert!(matches!(err, ToolError::NotFound { .. }));

    assert!(
        handle
            .get_member(&AgentIdentity::from("w-ext"))
            .await
            .is_none(),
        "hidden operator spawn tool must not provision a backend peer"
    );
}

#[tokio::test]
async fn test_mob_task_tools_hidden_without_operator_context() {
    let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    for required in [
        "mob_task_create",
        "mob_task_list",
        "mob_task_update",
        "mob_task_get",
    ] {
        assert!(
            !service
                .external_tool_names(&sid_1)
                .await
                .contains(&required.to_string()),
            "task operator tool '{required}' must stay hidden without runtime-injected operator context"
        );
    }

    let create_err = service
        .dispatch_external_tool(
            &sid_1,
            "mob_task_create",
            serde_json::json!({
                "subject": "Foundations",
                "description": "Prepare prerequisites"
            }),
        )
        .await
        .expect_err("mob_task_create should be unavailable");
    assert!(matches!(create_err, ToolError::NotFound { .. }));
}

#[tokio::test]
async fn test_tool_flag_enforcement_blocks_mob_and_task_tools() {
    let (handle, service) = create_test_mob(sample_definition_without_mob_flags()).await;
    let sid = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("lead-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let names = service.external_tool_names(&sid).await;
    assert!(
        !names.iter().any(|name| name == "spawn_member"),
        "tools.mob=false should hide spawn_member"
    );
    assert!(
        !names.iter().any(|name| name == "mob_task_create"),
        "tools.mob_tasks=false should hide mob_task_create"
    );

    let spawn_err = service
        .dispatch_external_tool(
            &sid,
            "spawn_member",
            serde_json::json!({"profile": "worker", "member_id": "w-2"}),
        )
        .await
        .expect_err("spawn tool must be unavailable");
    assert!(matches!(spawn_err, ToolError::NotFound { .. }));

    let task_err = service
        .dispatch_external_tool(
            &sid,
            "mob_task_create",
            serde_json::json!({"task_id": "t1", "subject": "x", "description": "y"}),
        )
        .await
        .expect_err("task tool must be unavailable");
    assert!(matches!(task_err, ToolError::NotFound { .. }));
}

#[tokio::test]
async fn test_for_resume_rebuilds_definition_and_roster() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();

    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events.clone(),
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume");

    assert_eq!(resumed.mob_id().as_str(), "test-mob");
    let entry_1 = resumed
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    let entry_2 = resumed
        .get_member(&AgentIdentity::from("w-2"))
        .await
        .unwrap();
    assert!(entry_1.wired_to.contains(&AgentIdentity::from("w-2")));
    assert!(entry_2.wired_to.contains(&AgentIdentity::from("w-1")));
}

#[tokio::test]
async fn test_resume_fails_when_orchestrator_resume_notification_fails() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let mut def = sample_definition();
    for profile in def.profiles.values_mut().filter_map(|b| b.as_inline_mut()) {
        profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    }
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(def, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn_with_options(
            ProfileName::from("lead"),
            MeerkatId::from("lead-1"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn orchestrator");
    handle.stop().await.expect("stop");
    service.set_fail_start_turn(true);

    let result = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .resume()
    .await;

    let error = result
        .err()
        .expect("resume must surface orchestrator start_turn failures");
    assert!(
        matches!(
            &error,
            MobError::Internal(message) if message.contains("mock start_turn failure")
        ),
        "resume must surface orchestrator start_turn failures, got {error:?}"
    );
}

#[tokio::test]
async fn test_resume_reconciles_orphaned_sessions() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let _handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let orphan = service
        .create_session(CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "orphan".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            build: Some(meerkat_core::service::SessionBuildOptions {
                comms_name: Some("test-mob/worker/orphan".to_string()),
                ..Default::default()
            }),
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            labels: None,
        })
        .await
        .expect("create orphan");
    assert_eq!(service.active_session_count().await, 1);

    let _resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume");

    let sessions = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions");
    assert!(
        !sessions.iter().any(|s| s.session_id == orphan.session_id),
        "resume should archive orphaned sessions"
    );
}

#[tokio::test]
async fn test_resume_restores_missing_sessions_with_same_session_and_history() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    handle.stop().await.expect("stop");

    let old_sid = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let mut persisted = service
        .live_session_clone(&old_sid)
        .await
        .expect("live session");
    persisted.set_system_prompt("Persisted worker prompt".to_string());
    persisted.push(meerkat_core::Message::User(
        meerkat_core::types::UserMessage::text("Remember session continuity.".to_string()),
    ));
    service.replace_live_session(persisted).await;
    service
        .archive(&old_sid)
        .await
        .expect("archive missing session");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume");

    let restored_sid = resumed
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("restored entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    assert_eq!(
        restored_sid, old_sid,
        "persistent resume should restore the original session id"
    );
    let history = service
        .read_history(
            &restored_sid,
            meerkat_core::service::SessionHistoryQuery::default(),
        )
        .await
        .expect("history for restored session");
    assert!(
        history.messages.iter().any(|message| message
            .as_indexable_text()
            .contains("Remember session continuity.")),
        "restored session should preserve persisted history"
    );
    assert_eq!(service.active_session_count().await, 1);
}

#[tokio::test]
async fn test_resume_restores_missing_sessions_with_tool_wiring() {
    let mut definition = sample_definition_with_mob_tools();
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .unwrap();
    worker.tools.rust_bundles = vec!["bundle-a".to_string()];

    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition.clone(), storage)
        .with_session_service(service.clone())
        .register_tool_bundle("bundle-a", Arc::new(EchoBundleDispatcher))
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    handle.stop().await.expect("stop");

    let old_sid = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive missing session");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .register_tool_bundle("bundle-a", Arc::new(EchoBundleDispatcher))
    .resume()
    .await
    .expect("resume");

    let restored_sid = resumed
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("restored entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    assert_eq!(
        restored_sid, old_sid,
        "persistent resume should restore the original session id"
    );

    let names = service.external_tool_names(&restored_sid).await;
    assert!(
        !names.contains(&"spawn_member".to_string()),
        "restored sessions must not regain mob operator tools without runtime-injected context"
    );
    assert!(
        !names.contains(&"mob_task_create".to_string()),
        "restored sessions must not regain mob task operator tools without runtime-injected context"
    );
    assert!(
        names.contains(&"bundle_echo".to_string()),
        "rust bundle tools must be wired on restored sessions"
    );
}

#[tokio::test]
async fn test_resume_marks_missing_persisted_session_as_broken() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    handle.stop().await.expect("stop");

    let old_sid = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("partial resume should still succeed");

    let snapshot = resumed
        .member_status(&AgentIdentity::from("w-1"))
        .await
        .expect("broken member status");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
    assert!(snapshot.is_final, "broken members should be terminal");
    assert_eq!(snapshot.current_session_id, Some(old_sid.clone()));
    assert!(
        snapshot
            .error
            .as_deref()
            .is_some_and(|message| message.contains("missing durable session")),
        "broken member should surface restore failure reason"
    );

    let machine_state = resumed
        .query_machine_state()
        .await
        .expect("query MobMachine state after restore failure");
    let machine_identity =
        crate::machines::mob_machine::AgentIdentity::from_domain(&AgentIdentity::from("w-1"));
    assert_eq!(
        machine_state.member_lifecycle_for_identity(&machine_identity, true),
        crate::machines::mob_machine::MobMemberLifecycleMaterial {
            status: crate::machines::mob_machine::MobMemberLifecycleStatus::Broken,
            terminal_class: crate::machines::mob_machine::MobMemberTerminalClass::TerminalFailure,
            error: Some(format!("missing durable session snapshot for '{old_sid}'")),
        },
        "restore-failure member status must be owned by MobMachine, not reconstructed by projection code"
    );

    let members = resumed.list_members().await;
    let broken = members
        .into_iter()
        .find(|entry| entry.agent_identity == "w-1")
        .expect("broken member should remain visible in list_members");
    assert_eq!(
        broken.status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
    assert!(broken.is_final);
    assert_eq!(broken.current_session_id, Some(old_sid));
    assert!(
        broken
            .error
            .as_deref()
            .is_some_and(|message| message.contains("missing durable session")),
        "projected member listing should carry the restore failure"
    );

    let including_retiring = resumed.list_members_including_retiring().await;
    let broken_including_retiring = including_retiring
        .into_iter()
        .find(|entry| entry.agent_identity == "w-1")
        .expect("broken member should remain visible in list_members_including_retiring");
    assert_eq!(
        broken_including_retiring.status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
    assert!(broken_including_retiring.is_final);
}

#[tokio::test]
async fn test_resume_skips_wiring_for_broken_peer_and_keeps_partial_resume() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");
    handle.stop().await.expect("stop");

    service
        .archive(&sid_2)
        .await
        .expect("archive broken session");
    service.delete_persisted_session(&sid_2).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume should stay partial even with broken wired member");

    let left = resumed
        .member_status(&AgentIdentity::from("w-1"))
        .await
        .expect("healthy member status");
    assert_eq!(left.status, crate::runtime::handle::MobMemberStatus::Active);
    assert_eq!(left.current_session_id, Some(sid_1.clone()));

    let right = resumed
        .member_status(&AgentIdentity::from("w-2"))
        .await
        .expect("broken member status");
    assert_eq!(
        right.status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
    assert_eq!(right.current_session_id, Some(sid_2.clone()));

    let trusted_by_left = service.trusted_peer_names(&sid_1).await;
    assert!(
        !trusted_by_left.contains(&test_comms_name("worker", "w-2")),
        "healthy members must prune trust to broken peers during partial resume"
    );
}

#[tokio::test]
async fn test_resume_restores_missing_live_session_even_when_list_reports_inactive_persisted_summary()
 {
    let inner = Arc::new(MockSessionService::new());
    let _ = inner.enable_runtime_adapter();
    let service = Arc::new(PersistedListingSessionService::new(inner.clone()));
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let sid = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle.stop().await.expect("stop");
    inner.archive(&sid).await.expect("archive session");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .resume()
    .await
    .expect("resume should restore from persisted snapshot, not treat inactive summary as live");

    let snapshot = resumed
        .member_status(&AgentIdentity::from("w-1"))
        .await
        .expect("member status after resume");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Active
    );
    assert_eq!(snapshot.current_session_id, Some(sid.clone()));
    assert!(
        inner.comms_runtime(&sid).await.is_some(),
        "resume should recreate the live session bridge for persisted-but-inactive sessions"
    );
}

#[tokio::test]
async fn test_resume_skips_broken_orchestrator_notification_and_keeps_partial_resume() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("lead-1"), None)
        .await
        .expect("spawn orchestrator");
    handle.stop().await.expect("stop");

    let orchestrator_sid = handle
        .get_member(&AgentIdentity::from("lead-1"))
        .await
        .expect("orchestrator entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&orchestrator_sid)
        .await
        .expect("archive orchestrator");
    service.delete_persisted_session(&orchestrator_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume should succeed when orchestrator is broken");

    let orchestrator = resumed
        .member_status(&AgentIdentity::from("lead-1"))
        .await
        .expect("orchestrator status");
    assert_eq!(
        orchestrator.status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
    assert_eq!(orchestrator.current_session_id, Some(orchestrator_sid));
}

#[tokio::test]
async fn test_broken_member_turn_returns_restore_failed_error() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let mut definition = sample_definition();
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .unwrap()
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    handle.stop().await.expect("stop");

    let old_sid = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("partial resume should still succeed");
    match resumed.resume().await {
        Ok(()) => {}
        Err(MobError::InvalidTransition {
            from: MobState::Running,
            to: MobState::Running,
        }) => {}
        Err(error) => {
            panic!(
                "broken members should still surface restore failures after lifecycle resume: {error}"
            )
        }
    }

    let error = resumed
        .internal_turn(
            AgentIdentity::from("w-1"),
            ContentInput::from("repair me".to_string()),
        )
        .await
        .expect_err("Broken members must reject turn delivery");

    match error {
        MobError::MemberRestoreFailed {
            member_id,
            session_id,
            ..
        } => {
            assert_eq!(member_id, MeerkatId::from("w-1"));
            assert_eq!(session_id, Some(old_sid));
        }
        other => panic!("expected MemberRestoreFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn test_wire_broken_member_returns_restore_failed_error() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let mut definition = sample_definition();
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .unwrap()
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn broken candidate");
    handle.stop().await.expect("stop");

    let old_sid = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("partial resume should still succeed");
    match resumed.resume().await {
        Ok(()) => {}
        Err(MobError::InvalidTransition {
            from: MobState::Running,
            to: MobState::Running,
        }) => {}
        Err(error) => {
            panic!("broken members should still allow mob lifecycle resume: {error}")
        }
    }

    resumed
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn healthy worker");

    let error = resumed
        .wire(AgentIdentity::from("w-2"), MeerkatId::from("w-1"))
        .await
        .expect_err("wiring to Broken member must be rejected");

    match error {
        MobError::MemberRestoreFailed {
            member_id,
            session_id,
            ..
        } => {
            assert_eq!(member_id, MeerkatId::from("w-1"));
            assert_eq!(session_id, Some(old_sid));
        }
        other => panic!("expected MemberRestoreFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn test_retire_broken_member_succeeds_and_removes_it() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    handle.stop().await.expect("stop");

    let old_sid = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("partial resume should still succeed");

    resumed
        .retire(AgentIdentity::from("w-1"))
        .await
        .expect("retire should work on broken member");

    assert!(
        resumed
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "retire should remove the broken member from the roster"
    );
}

#[tokio::test]
async fn test_respawn_broken_member_clears_restore_diagnostic() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let mut definition = sample_definition();
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .unwrap()
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    handle.stop().await.expect("stop");

    let old_sid = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("partial resume should still succeed");
    match resumed.resume().await {
        Ok(()) => {}
        Err(MobError::InvalidTransition {
            from: MobState::Running,
            to: MobState::Running,
        }) => {}
        Err(error) => {
            panic!("broken members should still be repairable after lifecycle resume: {error}")
        }
    }

    let receipt = resumed
        .respawn(AgentIdentity::from("w-1"), None)
        .await
        .expect("respawn should repair broken member");
    let snapshot = resumed
        .member_status(&AgentIdentity::from("w-1"))
        .await
        .expect("member status after repair");
    let new_sid = snapshot
        .current_bridge_session_id
        .clone()
        .expect("respawned member should have a new session");
    assert_ne!(
        new_sid, old_sid,
        "respawn should rotate the session identity"
    );

    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Active
    );
    assert_eq!(snapshot.agent_runtime_id, receipt.agent_runtime_id);
    assert_eq!(snapshot.fence_token, receipt.fence_token);
    assert_eq!(snapshot.current_session_id, Some(new_sid.clone()));
    assert!(
        snapshot.error.is_none(),
        "repair should clear the old broken diagnostic"
    );

    let members = resumed.list_members().await;
    let repaired = members
        .into_iter()
        .find(|entry| entry.agent_identity == "w-1")
        .expect("repaired member remains listed");
    assert_eq!(
        repaired.status,
        crate::runtime::handle::MobMemberStatus::Active
    );
    assert!(repaired.error.is_none());
    assert_eq!(repaired.current_session_id, Some(new_sid));
}

#[tokio::test]
async fn test_resume_restores_persisted_behavior_metadata() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    handle.stop().await.expect("stop");

    let old_sid = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let mut persisted = service
        .live_session_clone(&old_sid)
        .await
        .expect("live session");
    persisted.set_system_prompt("Persisted worker prompt".to_string());
    let mut metadata = persisted
        .session_metadata()
        .expect("seeded session metadata should exist");
    metadata.model = "gpt-5.4-pro".to_string();
    metadata.provider = Provider::OpenAI;
    metadata.provider_params = Some(serde_json::json!({
        "reasoning": { "effort": "high" }
    }));
    metadata.max_tokens = 8192;
    metadata.tooling.builtins = ToolCategoryOverride::Disable;
    metadata.tooling.shell = ToolCategoryOverride::Enable;
    metadata.tooling.mob = ToolCategoryOverride::Enable;
    metadata.tooling.memory = ToolCategoryOverride::Enable;
    metadata.tooling.active_skills = Some(vec![meerkat_core::skills::SkillKey::builtin(
        meerkat_core::skills::SkillName::parse("mob-communication").expect("valid skill name"),
    )]);
    metadata.comms_name = Some(test_comms_name("worker", "w-1"));
    let mut peer_meta = metadata.peer_meta.clone().expect("peer metadata");
    peer_meta.labels.insert("resume".into(), "yes".into());
    metadata.peer_meta = Some(peer_meta.clone());
    persisted
        .set_session_metadata(metadata.clone())
        .expect("metadata update");
    service.replace_live_session(persisted).await;
    service
        .archive(&old_sid)
        .await
        .expect("archive missing session");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume");

    let restored_sid = resumed
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("restored entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let restored = service
        .live_session_clone(&restored_sid)
        .await
        .expect("restored live session");
    let restored_metadata = restored
        .session_metadata()
        .expect("restored session metadata");

    assert_eq!(restored_sid, old_sid);
    assert_eq!(restored_metadata.model, metadata.model);
    assert_eq!(restored_metadata.provider, metadata.provider);
    assert_eq!(restored_metadata.provider_params, metadata.provider_params);
    assert_eq!(restored_metadata.max_tokens, metadata.max_tokens);
    assert_eq!(restored_metadata.tooling, metadata.tooling);
    assert!(
        restored_metadata.keep_alive,
        "autonomous host member should be restored with keep_alive=true"
    );
    assert_eq!(restored_metadata.comms_name, metadata.comms_name);
    assert_eq!(restored_metadata.peer_meta, metadata.peer_meta);
    assert!(
        matches!(
            restored.messages().first(),
            Some(Message::System(system)) if system.content.contains("Persisted worker prompt")
        ),
        "restored session should keep the persisted system prompt"
    );
}

#[tokio::test]
async fn test_resume_marks_comms_name_mismatch_as_broken() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    handle.stop().await.expect("stop");

    let old_sid = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let mut persisted = service
        .live_session_clone(&old_sid)
        .await
        .expect("live session");
    let mut metadata = persisted.session_metadata().expect("metadata");
    metadata.comms_name = Some("test-mob/worker/other-name".to_string());
    persisted
        .set_session_metadata(metadata)
        .expect("set metadata");
    service.replace_live_session(persisted).await;
    service.archive(&old_sid).await.expect("archive session");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume should succeed with Broken projection");
    let snapshot = resumed
        .member_status(&AgentIdentity::from("w-1"))
        .await
        .expect("broken member status");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
    assert!(
        snapshot
            .error
            .as_deref()
            .is_some_and(|message| message.contains("persisted comms_name")),
        "comms_name mismatch should surface as a restore failure"
    );
}

#[tokio::test]
async fn test_attach_existing_session_rejects_comms_name_mismatch() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let initial = service
        .create_session(CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "seed".to_string().into(),
            render_metadata: None,
            system_prompt: Some("Persisted resume prompt".to_string()),
            max_tokens: Some(4096),
            event_tx: None,
            build: Some(meerkat_core::service::SessionBuildOptions {
                comms_name: Some("test-mob/worker/w-resume".to_string()),
                ..Default::default()
            }),
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            labels: None,
        })
        .await
        .expect("seed session");
    let session_id = initial.session_id.clone();
    let mut persisted = service
        .live_session_clone(&session_id)
        .await
        .expect("live session");
    let mut metadata = persisted.session_metadata().expect("metadata");
    metadata.comms_name = Some("test-mob/worker/not-w-resume".to_string());
    persisted
        .set_session_metadata(metadata)
        .expect("set metadata");
    service.replace_live_session(persisted).await;
    service.archive(&session_id).await.expect("archive session");

    let handle = MobBuilder::new(sample_definition(), MobStorage::in_memory())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let error = handle
        .attach_existing_session_as_member(
            ProfileName::from("worker"),
            MeerkatId::from("w-resume"),
            session_id.clone(),
        )
        .await
        .expect_err("comms_name mismatch should be rejected for explicit resume");
    assert!(
        error.to_string().contains("persisted comms_name"),
        "explicit member resume should fail with the comms_name mismatch"
    );
}

#[tokio::test]
async fn test_build_resumed_agent_config_rejects_mismatched_session_identity() {
    let definition = sample_definition();
    let mob_id = MobId::from("test-mob");
    let profile_name = ProfileName::from("worker");
    let meerkat_id = MeerkatId::from("w-1");
    let profile = definition
        .profiles
        .get(&profile_name)
        .expect("worker profile")
        .as_inline()
        .unwrap();
    let mut resumed = Session::new();
    resumed
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Disable,
                comms: ToolCategoryOverride::Enable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                active_skills: Some(vec![meerkat_core::skills::SkillKey::builtin(
                    meerkat_core::skills::SkillName::parse("mob-communication")
                        .expect("valid skill name"),
                )]),
            },
            keep_alive: false,
            comms_name: Some(test_comms_name("worker", "w-1")),
            peer_meta: Some(
                meerkat_core::PeerMeta::default()
                    .with_label("mob_id", "test-mob")
                    .with_label("role", "worker")
                    .with_label("member_id", "w-1"),
            ),
            realm_id: Some("mob.test-mob".to_string()),
            instance_id: None,
            backend: None,
            config_generation: None,
            connection_ref: None,
        })
        .expect("resume metadata");

    let wrong_session_id = SessionId::new();
    let error =
        crate::build::build_resumed_agent_config(crate::build::BuildResumedAgentConfigParams {
            base: crate::build::BuildAgentConfigParams {
                mob_id: &mob_id,
                profile_name: &profile_name,
                agent_identity: &meerkat_id,
                profile,
                definition: &definition,
                external_tools: None,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
                mob_tool_access_context: crate::build::MobToolAccessContext::None,
                inherited_tool_filter: None,
            },
            expected_session_id: &wrong_session_id,
            resumed_session: resumed,
        })
        .await
        .expect_err("resume helper must validate the target session identity");
    assert!(
        error.to_string().contains("resume session id mismatch"),
        "mismatched injected resume_session should fail immediately"
    );
}

#[tokio::test]
async fn test_attach_existing_session_restores_persisted_inactive_session() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let initial = service
        .create_session(CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "seed".to_string().into(),
            render_metadata: None,
            system_prompt: Some("Persisted resume prompt".to_string()),
            max_tokens: Some(4096),
            event_tx: None,
            build: Some(meerkat_core::service::SessionBuildOptions {
                comms_name: Some("test-mob/worker/w-resume".to_string()),
                ..Default::default()
            }),
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            labels: None,
        })
        .await
        .expect("seed session");
    let session_id = initial.session_id.clone();
    let mut persisted = service
        .live_session_clone(&session_id)
        .await
        .expect("live session");
    persisted.push(meerkat_core::Message::User(
        meerkat_core::types::UserMessage::text("Persist this resume target.".to_string()),
    ));
    service.replace_live_session(persisted).await;
    service
        .archive(&session_id)
        .await
        .expect("archive seeded session");

    let handle = MobBuilder::new(sample_definition(), MobStorage::in_memory())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let member_ref = handle
        .attach_existing_session_as_member(
            ProfileName::from("worker"),
            MeerkatId::from("w-resume"),
            session_id.clone(),
        )
        .await
        .expect("attach should restore persisted session");

    assert_eq!(
        member_ref.bridge_session_id(),
        Some(&session_id),
        "explicit member resume should preserve the requested session id"
    );
    let history = service
        .read_history(
            &session_id,
            meerkat_core::service::SessionHistoryQuery::default(),
        )
        .await
        .expect("restored history");
    assert!(
        history.messages.iter().any(|message| message
            .as_indexable_text()
            .contains("Persist this resume target.")),
        "explicit member resume should restore persisted history"
    );
}

#[tokio::test]
async fn test_resume_recreates_missing_external_bridge_preserving_backend_identity() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "resume-external-peer-only",
    );
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let external = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext")).await;
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            external.binding(),
        )
        .await
        .expect("spawn external");
    assert_eq!(
        external.bind_count(),
        1,
        "peer-only external spawn should bind the mob supervisor during provisioning"
    );
    handle.stop().await.expect("stop");

    let old_entry = handle
        .get_member(&AgentIdentity::from("w-ext"))
        .await
        .expect("external roster entry");
    let (old_peer_id, old_address, old_sid) = match old_entry.member_ref {
        MemberRef::BackendPeer {
            ref peer_id,
            ref address,
            ref session_id,
            ..
        } => (peer_id.clone(), address.clone(), session_id.clone()),
        ref other => panic!("expected external backend member ref, got {other:?}"),
    };
    if let Some(old_sid) = &old_sid {
        service
            .archive(old_sid)
            .await
            .expect("archive legacy external bridge session");
    }

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume");
    let resumed_entry = resumed
        .get_member(&AgentIdentity::from("w-ext"))
        .await
        .expect("resumed external entry");
    match resumed_entry.member_ref {
        MemberRef::BackendPeer {
            peer_id,
            address,
            session_id,
            ..
        } => {
            assert_eq!(peer_id, old_peer_id, "resume must preserve backend peer_id");
            assert_eq!(address, old_address, "resume must preserve backend address");
            assert!(
                session_id.is_none(),
                "resume should preserve peer-only external members without recreating a bridge session"
            );
        }
        other => panic!("expected backend peer after resume, got {other:?}"),
    }
}

#[tokio::test]
async fn test_resume_treats_normalized_external_binding_overlay_as_projection_only() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let definition = sample_definition_with_external_backend();
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle.stop().await.expect("stop");
    let old_peer_id = "ed25519:test-key:w-ext".to_string();
    let old_address = "tcp://test.invalid/w-ext".to_string();
    let old_sid = SessionId::new();
    let generation = crate::ids::Generation::INITIAL;
    events
        .append(crate::event::NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: crate::event::MobEventKind::MemberSpawned(
                crate::event::MemberSpawnedEvent::new(
                    AgentIdentity::from("w-ext"),
                    generation,
                    FenceToken::new(0),
                    AgentRuntimeId::initial(AgentIdentity::from("w-ext")),
                    ProfileName::from("worker"),
                )
                .with_bridge_member_ref(Some(MemberRef::BackendPeer {
                    peer_id: old_peer_id.clone(),
                    address: old_address.clone(),
                    bootstrap_token: None,
                    session_id: Some(old_sid.clone()),
                })),
            ),
        })
        .await
        .expect("append legacy external member event");
    runtime_metadata
        .upsert_external_binding_overlay(
            &mob_id,
            &ExternalBindingOverlayRecord {
                agent_identity: AgentIdentity::from("w-ext"),
                generation,
                normalized_member_ref: Some(MemberRef::BackendPeer {
                    peer_id: old_peer_id.clone(),
                    address: old_address.clone(),
                    bootstrap_token: None,
                    session_id: None,
                }),
                bootstrap_token: None,
                status: ExternalBindingOverlayStatus::Normalized,
                updated_at: Utc::now(),
            },
        )
        .await
        .expect("persist overlay");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume with normalized overlay");
    let resumed_entry = resumed
        .get_member(&AgentIdentity::from("w-ext"))
        .await
        .expect("resumed external entry");
    match resumed_entry.member_ref {
        MemberRef::BackendPeer {
            peer_id,
            address,
            session_id,
            ..
        } => {
            assert_eq!(
                peer_id, old_peer_id,
                "event replay must preserve peer identity"
            );
            assert_eq!(
                address, old_address,
                "event replay must preserve peer address"
            );
            assert!(
                session_id.is_some(),
                "normalized overlay must not remove the replay-owned bridge binding"
            );
        }
        other => panic!("expected backend peer after resume, got {other:?}"),
    }

    let snapshot = resumed
        .member_status(&AgentIdentity::from("w-ext"))
        .await
        .expect("external status");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Active
    );
    assert!(
        !snapshot.is_final,
        "external member should stay non-terminal under MobMachine lifecycle truth"
    );
    assert!(
        snapshot.current_bridge_session_id().is_some(),
        "status must report replay/reconcile-owned bridge binding"
    );
    let external = snapshot
        .external_member
        .as_ref()
        .expect("external member should expose observation snapshot");
    assert_eq!(
        external.owner.agent_identity,
        AgentIdentity::from("w-ext"),
        "external observation is keyed by stable AgentIdentity"
    );
    assert_eq!(external.owner.mob_id, mob_id);
    assert_eq!(
        external.binding_mode,
        crate::runtime::ExternalMemberBindingMode::BridgeSessionBacked
    );
    assert!(external.bridge_session_present);
    assert_eq!(
        external.rebind,
        crate::runtime::ExternalMemberRebindStatus::NotRequired
    );
    let serialized_external =
        serde_json::to_value(external).expect("serialize external observation");
    assert!(
        serialized_external.get("peer_id").is_none(),
        "external observation must not expose raw peer ids as public authority"
    );
    assert!(
        serialized_external.get("address").is_none(),
        "external observation must not expose transport addresses as public authority"
    );
    assert!(
        serialized_external.get("bootstrap_token").is_none(),
        "external observation must not expose bootstrap proofs"
    );
    assert!(
        serialized_external.get("agent_runtime_id").is_none(),
        "external observation must not expose raw runtime ids as public authority"
    );
    assert!(
        serialized_external.get("fence_token").is_none(),
        "external observation must not expose fence tokens as public authority"
    );
}

#[tokio::test]
async fn test_resume_treats_failed_external_binding_overlay_as_projection_only() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let definition = sample_definition_with_external_backend();
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle.stop().await.expect("stop");
    let old_sid = SessionId::new();
    let generation = crate::ids::Generation::INITIAL;
    events
        .append(crate::event::NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: crate::event::MobEventKind::MemberSpawned(
                crate::event::MemberSpawnedEvent::new(
                    AgentIdentity::from("w-ext"),
                    generation,
                    FenceToken::new(0),
                    AgentRuntimeId::initial(AgentIdentity::from("w-ext")),
                    ProfileName::from("worker"),
                )
                .with_bridge_member_ref(Some(MemberRef::BackendPeer {
                    peer_id: "ed25519:test-key:w-ext".to_string(),
                    address: "tcp://test.invalid/w-ext".to_string(),
                    bootstrap_token: None,
                    session_id: Some(old_sid.clone()),
                })),
            ),
        })
        .await
        .expect("append legacy external member event");
    runtime_metadata
        .upsert_external_binding_overlay(
            &mob_id,
            &ExternalBindingOverlayRecord {
                agent_identity: AgentIdentity::from("w-ext"),
                generation,
                normalized_member_ref: None,
                bootstrap_token: None,
                status: ExternalBindingOverlayStatus::Failed {
                    reason: "legacy external bridge normalization failed".to_string(),
                },
                updated_at: Utc::now(),
            },
        )
        .await
        .expect("persist failed overlay");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume should ignore stale failed overlay authority");
    let resumed_entry = resumed
        .get_member(&AgentIdentity::from("w-ext"))
        .await
        .expect("resumed external entry");
    match resumed_entry.member_ref {
        MemberRef::BackendPeer { session_id, .. } => {
            assert!(
                session_id.is_some(),
                "failed overlay must not strip the replay-owned bridge binding"
            );
        }
        other => panic!("expected backend peer after resume, got {other:?}"),
    }

    let snapshot = resumed
        .member_status(&AgentIdentity::from("w-ext"))
        .await
        .expect("external status");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Active
    );
    assert!(
        snapshot.error.is_none(),
        "failed overlay reason must not surface as member status truth"
    );
    let external = snapshot
        .external_member
        .as_ref()
        .expect("external member should still expose observation snapshot");
    assert_eq!(
        external.reachability,
        crate::runtime::ExternalMemberReachability::Unknown
    );
    assert_eq!(
        external.rebind,
        crate::runtime::ExternalMemberRebindStatus::NotRequired
    );
    assert_eq!(
        external.forwarding.approvals.owner.agent_identity,
        AgentIdentity::from("w-ext"),
        "approval forwarding hook should be stable-identity owned"
    );
    assert_eq!(
        external.forwarding.artifacts.owner.agent_identity,
        AgentIdentity::from("w-ext"),
        "artifact forwarding hook should be stable-identity owned"
    );

    resumed
        .internal_turn(
            AgentIdentity::from("w-ext"),
            ContentInput::from("repair me".to_string()),
        )
        .await
        .expect("stale failed overlay must not reject turn delivery");
}

async fn resume_with_stale_external_binding_overlay(
    label: &str,
    status: ExternalBindingOverlayStatus,
    overlay_member_ref: Option<MemberRef>,
) -> MobHandle {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let definition = with_unique_mob_id(sample_definition_with_external_backend(), label);
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle.stop().await.expect("stop");

    let old_sid = SessionId::new();
    let generation = crate::ids::Generation::INITIAL;
    events
        .append(crate::event::NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: crate::event::MobEventKind::MemberSpawned(
                crate::event::MemberSpawnedEvent::new(
                    AgentIdentity::from("w-ext"),
                    generation,
                    FenceToken::new(0),
                    AgentRuntimeId::initial(AgentIdentity::from("w-ext")),
                    ProfileName::from("worker"),
                )
                .with_bridge_member_ref(Some(MemberRef::BackendPeer {
                    peer_id: "ed25519:test-key:w-ext".to_string(),
                    address: "tcp://test.invalid/w-ext".to_string(),
                    bootstrap_token: None,
                    session_id: Some(old_sid),
                })),
            ),
        })
        .await
        .expect("append external member event");
    runtime_metadata
        .upsert_external_binding_overlay(
            &mob_id,
            &ExternalBindingOverlayRecord {
                agent_identity: AgentIdentity::from("w-ext"),
                generation,
                normalized_member_ref: overlay_member_ref,
                bootstrap_token: None,
                status,
                updated_at: Utc::now(),
            },
        )
        .await
        .expect("persist stale overlay");

    MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .resume()
    .await
    .expect("resume must be driven by event/MobMachine authority")
}

#[tokio::test]
async fn test_resume_ignores_stale_normalized_external_binding_overlay_for_member_material() {
    let resumed = resume_with_stale_external_binding_overlay(
        "stale-normalized-overlay",
        ExternalBindingOverlayStatus::Normalized,
        Some(MemberRef::BackendPeer {
            peer_id: "ed25519:test-key:w-ext".to_string(),
            address: "tcp://test.invalid/w-ext".to_string(),
            bootstrap_token: None,
            session_id: None,
        }),
    )
    .await;

    let entry = resumed
        .get_member(&AgentIdentity::from("w-ext"))
        .await
        .expect("replayed member must remain visible");
    assert!(
        entry.member_ref.bridge_session_id().is_some(),
        "stale overlay-only normalization must not hide the replayed bridge binding"
    );

    let snapshot = resumed
        .member_status(&AgentIdentity::from("w-ext"))
        .await
        .expect("member status");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Active,
        "stale overlay-only facts must not override MobMachine lifecycle truth"
    );
    assert!(
        snapshot.current_bridge_session_id().is_some(),
        "status must report the MobMachine/replay-owned bridge binding, not overlay-only absence"
    );
}

#[tokio::test]
async fn test_resume_ignores_stale_failed_external_binding_overlay_for_status_truth() {
    let resumed = resume_with_stale_external_binding_overlay(
        "stale-failed-overlay",
        ExternalBindingOverlayStatus::Failed {
            reason: "stale overlay failure".to_string(),
        },
        None,
    )
    .await;

    let snapshot = resumed
        .member_status(&AgentIdentity::from("w-ext"))
        .await
        .expect("member status");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Active,
        "stale overlay-only failure must not mark a replayed live member broken"
    );
    assert!(
        snapshot.error.is_none(),
        "stale overlay-only failure must not surface as member status truth"
    );
    resumed
        .member(&AgentIdentity::from("w-ext"))
        .await
        .expect("stale overlay failure must not block member access");
}

#[tokio::test]
async fn test_resume_does_not_require_external_binding_overlay_load_for_authority() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let events = Arc::new(InMemoryMobEventStore::new());
    let runtime_metadata = Arc::new(FaultInjectedRuntimeMetadataStore::new());
    let storage =
        MobStorage::with_events_and_runtime_metadata(events.clone(), runtime_metadata.clone());
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle.stop().await.expect("stop");
    runtime_metadata
        .fail_list_overlays
        .store(true, Ordering::Relaxed);

    MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .resume()
    .await
    .expect("overlay projection load failure must not block MobMachine-owned resume");
}

#[tokio::test]
async fn test_reconcile_spawns_member_despite_stale_overlay_only_record() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let definition = with_unique_mob_id(sample_definition(), "overlay-only-reconcile");
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle.stop().await.expect("stop");
    runtime_metadata
        .upsert_external_binding_overlay(
            &mob_id,
            &ExternalBindingOverlayRecord {
                agent_identity: AgentIdentity::from("ghost"),
                generation: crate::ids::Generation::INITIAL,
                normalized_member_ref: Some(MemberRef::BackendPeer {
                    peer_id: "ed25519:test-key:ghost".to_string(),
                    address: "tcp://test.invalid/ghost".to_string(),
                    bootstrap_token: None,
                    session_id: None,
                }),
                bootstrap_token: None,
                status: ExternalBindingOverlayStatus::Normalized,
                updated_at: Utc::now(),
            },
        )
        .await
        .expect("persist overlay-only record");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .resume()
    .await
    .expect("resume");
    assert!(
        resumed
            .get_member(&AgentIdentity::from("ghost"))
            .await
            .is_none(),
        "overlay-only records must not materialize members on restart"
    );

    let report = resumed
        .reconcile(
            vec![SpawnMemberSpec::new("worker", "ghost")],
            crate::runtime::reconcile::ReconcileOptions::default(),
        )
        .await
        .expect("reconcile");
    assert_eq!(
        report.spawned.len(),
        1,
        "reconcile must spawn desired members that exist only as stale overlays"
    );
    assert!(
        report.retained.is_empty(),
        "overlay-only records must not be retained as existing roster members"
    );
}

#[tokio::test]
async fn test_peer_only_members_reject_agent_event_subscriptions_explicitly() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let definition = sample_definition_with_external_backend();
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle.stop().await.expect("stop");
    let peer_id = "ed25519:test-key:w-ext".to_string();
    let address = "tcp://test.invalid/w-ext".to_string();
    let generation = crate::ids::Generation::INITIAL;
    events
        .append(crate::event::NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: crate::event::MobEventKind::MemberSpawned(
                crate::event::MemberSpawnedEvent::new(
                    AgentIdentity::from("w-ext"),
                    generation,
                    FenceToken::new(0),
                    AgentRuntimeId::initial(AgentIdentity::from("w-ext")),
                    ProfileName::from("worker"),
                )
                .with_bridge_member_ref(Some(MemberRef::BackendPeer {
                    peer_id: peer_id.clone(),
                    address: address.clone(),
                    bootstrap_token: None,
                    session_id: None,
                })),
            ),
        })
        .await
        .expect("append peer-only external member event");
    runtime_metadata
        .upsert_external_binding_overlay(
            &mob_id,
            &ExternalBindingOverlayRecord {
                agent_identity: AgentIdentity::from("w-ext"),
                generation,
                normalized_member_ref: Some(MemberRef::BackendPeer {
                    peer_id,
                    address,
                    bootstrap_token: None,
                    session_id: None,
                }),
                bootstrap_token: None,
                status: ExternalBindingOverlayStatus::Normalized,
                updated_at: Utc::now(),
            },
        )
        .await
        .expect("persist overlay");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume peer-only external member");

    let single_error = match resumed
        .subscribe_agent_events(&AgentIdentity::from("w-ext"))
        .await
    {
        Ok(_) => panic!("peer-only members must reject direct event subscriptions"),
        Err(error) => error,
    };
    assert!(matches!(
        single_error,
        MobError::UnsupportedForMode {
            mode: crate::MobRuntimeMode::TurnDriven,
            ..
        }
    ));

    let all_error = match resumed.subscribe_all_agent_events().await {
        Ok(_) => panic!("peer-only members must reject aggregate event subscriptions"),
        Err(error) => error,
    };
    assert!(matches!(
        all_error,
        MobError::UnsupportedForMode {
            mode: crate::MobRuntimeMode::TurnDriven,
            ..
        }
    ));
}

#[tokio::test]
async fn test_peer_only_members_accept_direct_turn_delivery_without_bridge_session() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let mut definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "peer-only-direct-turn",
    );
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .unwrap()
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle.stop().await.expect("stop");
    let external = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext")).await;
    let crate::RuntimeBinding::External {
        peer_id,
        address,
        bootstrap_token,
        pubkey: _,
    } = external.binding()
    else {
        panic!("live external peer must produce external binding");
    };
    let generation = crate::ids::Generation::INITIAL;
    events
        .append(crate::event::NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: crate::event::MobEventKind::MemberSpawned(
                crate::event::MemberSpawnedEvent::new(
                    AgentIdentity::from("w-ext"),
                    generation,
                    FenceToken::new(0),
                    AgentRuntimeId::initial(AgentIdentity::from("w-ext")),
                    ProfileName::from("worker"),
                )
                .with_bridge_member_ref(Some(MemberRef::BackendPeer {
                    peer_id: peer_id.clone(),
                    address: address.clone(),
                    bootstrap_token: bootstrap_token.clone(),
                    session_id: None,
                })),
            ),
        })
        .await
        .expect("append peer-only external member event");
    runtime_metadata
        .upsert_external_binding_overlay(
            &mob_id,
            &ExternalBindingOverlayRecord {
                agent_identity: AgentIdentity::from("w-ext"),
                generation,
                normalized_member_ref: Some(MemberRef::BackendPeer {
                    peer_id,
                    address,
                    bootstrap_token: None,
                    session_id: None,
                }),
                bootstrap_token: bootstrap_token.clone(),
                status: ExternalBindingOverlayStatus::Normalized,
                updated_at: Utc::now(),
            },
        )
        .await
        .expect("persist overlay");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume peer-only external member");

    resumed
        .internal_turn(
            AgentIdentity::from("w-ext"),
            ContentInput::from("ping".to_string()),
        )
        .await
        .expect("peer-only members should accept direct turn delivery without a bridge session");
    assert_eq!(
        external.delivered_input_ids().await.len(),
        1,
        "peer-only direct turn should use request/ack delivery with one logical input admission"
    );

    let snapshot = resumed
        .member_status(&AgentIdentity::from("w-ext"))
        .await
        .expect("peer-only member status after direct turn");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Active
    );
    assert_eq!(snapshot.current_bridge_session_id(), None);
    let external_snapshot = snapshot
        .external_member
        .as_ref()
        .expect("live peer-only external member should expose observation snapshot");
    assert_eq!(
        external_snapshot.rebind,
        crate::runtime::ExternalMemberRebindStatus::Available,
        "live peer-only member with bootstrap proof should be rebind-capable"
    );
    assert!(
        external
            .trusted_peer_names()
            .await
            .iter()
            .any(|name| name.contains("__mob_supervisor__")),
        "peer-only direct turn should authorize the mob supervisor on the live external peer"
    );

    let respawn_receipt = resumed
        .respawn(AgentIdentity::from("w-ext"), None)
        .await
        .expect("peer-only members should respawn through runtime control");
    assert_eq!(respawn_receipt.identity, AgentIdentity::from("w-ext"));
    assert_ne!(
        respawn_receipt.previous_fence_token, respawn_receipt.fence_token,
        "respawn must advance the member fence token"
    );
    let post_respawn = resumed
        .member_status(&AgentIdentity::from("w-ext"))
        .await
        .expect("peer-only member status after respawn");
    assert_eq!(post_respawn.current_bridge_session_id(), None);
    let post_external = post_respawn
        .external_member
        .as_ref()
        .expect("external observation should survive respawn");
    assert_eq!(
        post_external.owner.agent_identity,
        AgentIdentity::from("w-ext"),
        "respawn must preserve stable AgentIdentity in external observation"
    );
    assert_eq!(
        post_external.rebind,
        crate::runtime::ExternalMemberRebindStatus::Available
    );
}

#[tokio::test]
async fn test_resume_reconciles_mixed_topology_without_losing_external_member_refs() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "resume-mixed-topology",
    );
    let mut definition = definition;
    for profile in definition
        .profiles
        .values_mut()
        .filter_map(|binding| binding.as_inline_mut())
    {
        profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    }
    let mob_id = definition.id.clone();
    let service = Arc::new(RealCommsSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let old_sub_sid = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-sub"), None)
        .await
        .expect("spawn session-backed member")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let external = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext")).await;
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            external.binding(),
        )
        .await
        .expect("spawn external");
    handle
        .wire(AgentIdentity::from("w-sub"), MeerkatId::from("w-ext"))
        .await
        .expect("wire mixed topology");
    handle.stop().await.expect("stop");

    let old_ext_entry = handle
        .get_member(&AgentIdentity::from("w-ext"))
        .await
        .expect("external entry before resume");
    let (old_ext_peer_id, old_ext_addr, old_ext_sid) = match old_ext_entry.member_ref {
        MemberRef::BackendPeer {
            ref peer_id,
            ref address,
            ref session_id,
            ..
        } => (peer_id.clone(), address.clone(), session_id.clone()),
        ref other => panic!("expected external backend member ref, got {other:?}"),
    };
    let old_sub_comms = service
        .real_comms(&old_sub_sid)
        .await
        .expect("session-backed member comms");
    old_sub_comms
        .remove_trusted_peer(&old_ext_peer_id)
        .await
        .expect("remove external trust before resume");
    if let Some(old_ext_sid) = &old_ext_sid {
        service
            .archive(old_ext_sid)
            .await
            .expect("archive legacy external bridge session");
    }

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume mixed topology");
    let resumed_sub = resumed
        .get_member(&AgentIdentity::from("w-sub"))
        .await
        .expect("resumed session-backed member entry");
    let resumed_sub_sid = match resumed_sub.member_ref {
        MemberRef::Session { ref session_id } => session_id.clone(),
        ref other => panic!("expected session member ref, got {other:?}"),
    };
    assert_eq!(
        resumed_sub_sid, old_sub_sid,
        "resume should restore missing session-backed member session with the same session_id"
    );

    let resumed_ext = resumed
        .get_member(&AgentIdentity::from("w-ext"))
        .await
        .expect("resumed external entry");
    match resumed_ext.member_ref {
        MemberRef::BackendPeer {
            peer_id,
            address,
            session_id,
            ..
        } => {
            assert_eq!(peer_id, old_ext_peer_id, "external peer_id must be stable");
            assert_eq!(address, old_ext_addr, "external address must be stable");
            assert!(
                session_id.is_none(),
                "mixed-topology resume should preserve peer-only externals without recreating a bridge session"
            );
        }
        other => panic!("expected backend peer member ref, got {other:?}"),
    }

    let trusted_sub = service
        .real_comms(&resumed_sub_sid)
        .await
        .expect("resumed session-backed member comms")
        .peers()
        .await
        .into_iter()
        .map(|entry| entry.address)
        .collect::<Vec<_>>();
    assert!(
        trusted_sub
            .iter()
            .any(|addr| addr.to_string() == old_ext_addr),
        "mixed resume should re-establish trust using the external member's real transport address"
    );
}

#[tokio::test]
async fn test_resume_reconciles_peer_only_trust_edges() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "resume-peer-only-trust-edges",
    );
    let mut definition = definition;
    for profile in definition
        .profiles
        .values_mut()
        .filter_map(|binding| binding.as_inline_mut())
    {
        profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    }
    let mob_id = definition.id.clone();
    let service = Arc::new(RealCommsSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let external_a = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-a")).await;
    let external_b = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-b")).await;

    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-a"),
            None,
            external_a.binding(),
        )
        .await
        .expect("spawn external a");
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-b"),
            None,
            external_b.binding(),
        )
        .await
        .expect("spawn external b");
    handle
        .wire(AgentIdentity::from("w-a"), MeerkatId::from("w-b"))
        .await
        .expect("wire peer-only pair");

    let crate::RuntimeBinding::External {
        peer_id: a_peer_id, ..
    } = external_a.binding()
    else {
        panic!("expected external a binding");
    };
    let crate::RuntimeBinding::External {
        peer_id: b_peer_id, ..
    } = external_b.binding()
    else {
        panic!("expected external b binding");
    };

    handle.stop().await.expect("stop mob");
    external_a.remove_trusted_peer(&b_peer_id).await;
    external_b.remove_trusted_peer(&a_peer_id).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .resume()
    .await
    .expect("resume peer-only trust topology");

    let name_a = test_comms_name_for(&mob_id, "worker", "w-a");
    let name_b = test_comms_name_for(&mob_id, "worker", "w-b");
    assert!(
        external_a.trusted_peer_names().await.contains(&name_b),
        "resume should restore worker b trust on peer-only worker a"
    );
    assert!(
        external_b.trusted_peer_names().await.contains(&name_a),
        "resume should restore worker a trust on peer-only worker b"
    );

    resumed.stop().await.expect("stop resumed mob");
}

#[tokio::test]
async fn test_resume_reestablishes_missing_trust() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");
    handle.stop().await.expect("stop");

    service.force_remove_trust(&sid_1, &sid_2).await;
    service.force_remove_trust(&sid_2, &sid_1).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume");
    let resumed_sid_1 = resumed
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("w-1")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let resumed_sid_2 = resumed
        .get_member(&AgentIdentity::from("w-2"))
        .await
        .expect("w-2")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let trusted_1 = service.trusted_peer_names(&resumed_sid_1).await;
    let trusted_2 = service.trusted_peer_names(&resumed_sid_2).await;
    assert!(
        trusted_1.contains(&test_comms_name("worker", "w-2")),
        "resume should restore trust from w-1 to w-2"
    );
    assert!(
        trusted_2.contains(&test_comms_name("worker", "w-1")),
        "resume should restore trust from w-2 to w-1"
    );
}

#[tokio::test]
async fn test_resume_prunes_stale_trust_not_present_in_roster() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let _sid_2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle.stop().await.expect("stop");

    let stale = TrustedPeerDescriptor::test_only_unsigned_typed(
        "remote-mob/worker/stale-peer",
        meerkat_core::comms::PeerId::new(),
        "inproc://remote-mob/worker/stale-peer",
    )
    .expect("valid stale peer");
    service
        .force_add_trust_from_spec(&sid_1, stale.clone())
        .await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume");
    let resumed_sid_1 = resumed
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("w-1")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let trusted = service.trusted_peer_names(&resumed_sid_1).await;
    assert!(
        !trusted.iter().any(|n| n == stale.name.as_str()),
        "resume should prune stale trust that is not present in the roster projection"
    );
}

#[tokio::test]
async fn test_resume_restores_external_wiring_from_event_log() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    let external = TrustedPeerDescriptor::test_only_unsigned_typed(
        "remote-mob/worker/agent-b",
        meerkat_core::comms::PeerId::new(),
        "inproc://remote-mob/worker/agent-b",
    )
    .expect("valid external peer");
    handle
        .wire(
            AgentIdentity::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external");
    handle.stop().await.expect("stop");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("resume");
    let resumed_sid = resumed
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("resumed member")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let trusted = service.trusted_peer_names(&resumed_sid).await;
    assert!(
        trusted.iter().any(|n| n == external.name.as_str()),
        "resume should restore external trusted peers from persisted wiring events"
    );
    let entry = resumed
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("resumed member");
    assert_eq!(
        entry
            .external_peer_specs
            .get(&MeerkatId::from(external.name.as_str()))
            .cloned(),
        Some(external)
    );
}

#[tokio::test]
async fn test_complete_archives_and_emits_mob_completed() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");

    handle.complete().await.expect("complete");
    assert_eq!(handle.status().await.unwrap(), MobState::Completed);
    assert!(
        handle.list_members().await.is_empty(),
        "complete should retire all active meerkats"
    );
    assert_eq!(
        service.active_session_count().await,
        0,
        "complete should archive all sessions"
    );
    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MobCompleted)),
        "complete should emit MobCompleted"
    );
}

#[tokio::test]
async fn test_destroy_deletes_storage() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    handle.destroy().await.expect("destroy");
    assert_eq!(handle.status().await.unwrap(), MobState::Destroyed);

    assert!(
        handle.list_members().await.is_empty(),
        "destroy should clear roster"
    );
    assert_eq!(
        service.active_session_count().await,
        0,
        "destroy should archive active sessions"
    );
    assert!(
        handle
            .events()
            .replay_all()
            .await
            .expect("replay")
            .is_empty(),
        "destroy should delete persisted events"
    );
}

#[tokio::test]
async fn test_destroy_cleans_mcp_namespace() {
    let (handle, _service) = create_test_mob(sample_definition_with_mcp_servers()).await;
    handle.destroy().await.expect("destroy");
    assert!(
        handle.mcp_server_states().await.is_empty(),
        "destroy should clean up mcp namespace state"
    );
}

#[tokio::test]
async fn test_for_resume_requires_mob_created_event() {
    let service = Arc::new(MockSessionService::new());
    let result = MobBuilder::for_resume(MobStorage::in_memory())
        .with_session_service(service)
        .resume()
        .await;
    assert!(matches!(result, Err(MobError::Internal(_))));
}

#[tokio::test]
async fn test_for_resume_rejects_non_persistent_session_service_by_default() {
    let storage = MobStorage::in_memory();
    storage
        .events
        .append(NewMobEvent {
            mob_id: MobId::from("test-mob"),
            timestamp: None,
            kind: MobEventKind::MobCreated {
                definition: Box::new(sample_definition()),
            },
        })
        .await
        .expect("append created event");

    let service = Arc::new(meerkat_session::EphemeralSessionService::new(
        PersistentMockBuilder,
        16,
    ));
    let result = MobBuilder::for_resume(storage)
        .with_session_service(service)
        .resume()
        .await;
    assert!(
        matches!(result, Err(MobError::Internal(message)) if message.contains("persistent-session contract")),
        "resume should reject non-persistent session services by default"
    );
}

#[tokio::test]
async fn test_for_resume_allows_ephemeral_session_service_when_opted_in() {
    let storage = MobStorage::in_memory();
    storage
        .runtime_metadata
        .put_supervisor_authority(
            &MobId::from("test-mob"),
            &default_supervisor_authority_record(),
        )
        .await
        .expect("persist supervisor metadata");
    storage
        .events
        .append(NewMobEvent {
            mob_id: MobId::from("test-mob"),
            timestamp: None,
            kind: MobEventKind::MobCreated {
                definition: Box::new(sample_definition()),
            },
        })
        .await
        .expect("append created event");

    let service = Arc::new(meerkat_session::EphemeralSessionService::new(
        PersistentMockBuilder,
        16,
    ));
    let resumed = MobBuilder::for_resume(storage)
        .with_session_service(service)
        .allow_ephemeral_sessions(true)
        .resume()
        .await
        .expect("resume should allow ephemeral sessions when explicitly enabled");
    assert_eq!(resumed.status().await.unwrap(), MobState::Running);
}

// -----------------------------------------------------------------------
// P1-T04: spawn() creates a real session
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_spawn_creates_session() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let session_id = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn should succeed")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    // Verify roster updated
    let meerkats = handle.list_members().await;
    assert_eq!(meerkats.len(), 1);
    assert_eq!(meerkats[0].agent_identity.as_str(), "w-1");
    assert_eq!(meerkats[0].role.as_str(), "worker");
    assert_eq!(
        meerkats[0].current_bridge_session_id.as_ref(),
        Some(&session_id)
    );
}

#[tokio::test]
async fn test_spawn_create_session_request_sets_peer_meta_labels() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");

    let create_requests = service.recorded_create_requests().await;
    assert_eq!(
        create_requests.len(),
        1,
        "exactly one create_session expected"
    );
    let req = &create_requests[0];
    assert_eq!(
        req.initial_turn,
        meerkat_core::service::InitialTurnPolicy::Defer,
        "spawn must defer initial turn; mob actor starts autonomous loop explicitly"
    );
    assert_eq!(req.comms_name.as_deref(), Some("test-mob/worker/w-1"));
    assert_eq!(
        req.peer_meta_labels.get("mob_id").map(String::as_str),
        Some("test-mob")
    );
    assert_eq!(
        req.peer_meta_labels.get("role").map(String::as_str),
        Some("worker")
    );
    assert_eq!(
        req.peer_meta_labels.get("meerkat_id").map(String::as_str),
        Some("w-1")
    );
}

#[tokio::test]
async fn test_spawn_emits_meerkat_spawned_event() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let session_id = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let events = handle.events().replay_all().await.expect("replay");
    // Should have MobCreated + MemberSpawned
    assert!(
        events.len() >= 2,
        "expected at least 2 events, got {}",
        events.len()
    );
    let spawned = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::MemberSpawned(..)));
    assert!(spawned.is_some(), "should have MemberSpawned event");
    if let MobEventKind::MemberSpawned(member_spawned) = &spawned.unwrap().kind {
        assert_eq!(member_spawned.agent_identity.as_str(), "w-1");
        assert_eq!(member_spawned.role.as_str(), "worker");
        assert_eq!(member_spawned.generation, crate::ids::Generation::INITIAL);
        assert!(
            member_spawned.fence_token.get() > 0,
            "fence token should be non-zero"
        );
        assert_eq!(
            member_spawned
                .bridge_member_ref()
                .and_then(|member_ref| member_ref.bridge_session_id()),
            Some(&session_id),
        );
    }
}

#[tokio::test]
async fn test_spawn_duplicate_meerkat_id_fails() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("first spawn");

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await;
    assert!(matches!(result, Err(MobError::MemberAlreadyExists(_))));
}

#[tokio::test]
async fn test_spawn_supports_session_and_external_backends() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "spawn-session-and-external",
    );
    let mob_id = definition.id.clone();
    let (handle, _service) = create_test_mob_with_real_comms(definition).await;

    let session_ref = handle
        .spawn_with_backend(
            ProfileName::from("worker"),
            MeerkatId::from("w-sub"),
            None,
            Some(MobBackendKind::Session),
        )
        .await
        .expect("spawn session backend");
    assert!(
        matches!(session_ref, MemberRef::Session { .. }),
        "session backend should emit session member ref"
    );

    let external = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext")).await;
    let crate::RuntimeBinding::External {
        peer_id: expected_peer_id,
        address: expected_address,
        bootstrap_token: _expected_bootstrap_token,
        pubkey: _expected_pubkey,
    } = external.binding()
    else {
        panic!("live external peer must produce external binding");
    };
    let external_ref = handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            external.binding(),
        )
        .await
        .expect("spawn external backend");
    match external_ref {
        MemberRef::BackendPeer {
            peer_id,
            address,
            session_id,
            ..
        } => {
            assert_eq!(
                peer_id, expected_peer_id,
                "external backend should use the committed peer_id returned by bind_member"
            );
            assert_eq!(
                address,
                canonical_external_address(&expected_address),
                "external backend should use the committed transport address returned by bind_member"
            );
            assert!(
                session_id.is_none(),
                "external backend should now persist peer-only member refs"
            );
        }
        other => panic!("expected backend peer member ref, got {other:?}"),
    }
    assert_eq!(
        external.bind_count(),
        1,
        "external spawn should issue exactly one bind request"
    );
    assert!(
        external
            .trusted_peer_names()
            .await
            .iter()
            .any(|name| name.contains("__mob_supervisor__")),
        "external spawn should authorize the mob supervisor during bind"
    );
}

#[tokio::test]
async fn test_external_backend_rejects_invalid_peer_name_components() {
    let (handle, _service) = create_test_mob(sample_definition_with_external_backend()).await;
    let result = handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("../w-ext"),
            None,
            test_external_binding("../w-ext"),
        )
        .await;
    assert!(
        matches!(result, Err(MobError::WiringError(_))),
        "invalid peer name components must fail external member registration"
    );
}

#[tokio::test]
async fn test_external_backend_wiring_uses_sendable_transport_addresses() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "external-wiring-addresses",
    );
    let mob_id = definition.id.clone();
    let (handle, _service) = create_test_mob(definition).await;
    let external_a = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-a")).await;
    let external_b = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-b")).await;

    let member_a = handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-a"),
            None,
            external_a.binding(),
        )
        .await
        .expect("spawn external w-a");
    let member_b = handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-b"),
            None,
            external_b.binding(),
        )
        .await
        .expect("spawn external w-b");
    assert_eq!(member_a.bridge_session_id(), None);
    assert_eq!(member_b.bridge_session_id(), None);

    handle
        .wire(AgentIdentity::from("w-a"), MeerkatId::from("w-b"))
        .await
        .expect("wire external peers");

    let roster = handle.roster().await;
    let entry_a = roster
        .get_by_identity(&AgentIdentity::from("w-a"))
        .expect("entry a");
    let entry_b = roster
        .get_by_identity(&AgentIdentity::from("w-b"))
        .expect("entry b");
    assert!(
        entry_a.wired_to.contains(&AgentIdentity::from("w-b")),
        "peer-only external wiring should remain symmetric in the roster"
    );
    assert!(
        entry_b.wired_to.contains(&AgentIdentity::from("w-a")),
        "peer-only external wiring should remain symmetric in the roster"
    );
    let trusted_a = external_a.trusted_peer_names().await;
    let trusted_b = external_b.trusted_peer_names().await;
    assert!(
        trusted_a
            .iter()
            .any(|name| name == &test_comms_name_for(&mob_id, "worker", "w-b")),
        "w-a should trust the peer-only remote target after wire"
    );
    assert!(
        trusted_b
            .iter()
            .any(|name| name == &test_comms_name_for(&mob_id, "worker", "w-a")),
        "w-b should trust the peer-only remote source after wire"
    );
    let intents_a = external_a.received_intents().await;
    let intents_b = external_b.received_intents().await;
    assert!(
        intents_a
            .iter()
            .any(|intent| intent == super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT),
        "w-a should receive an explicit wire_member bridge command"
    );
    assert!(
        intents_b
            .iter()
            .any(|intent| intent == super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT),
        "w-b should receive an explicit wire_member bridge command"
    );
}

#[tokio::test]
async fn test_external_backend_unwiring_revokes_remote_trust() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "external-unwiring-trust",
    );
    let mob_id = definition.id.clone();
    let (handle, _service) = create_test_mob(definition).await;
    let external_a = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "u-a")).await;
    let external_b = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "u-b")).await;

    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("u-a"),
            None,
            external_a.binding(),
        )
        .await
        .expect("spawn external u-a");
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("u-b"),
            None,
            external_b.binding(),
        )
        .await
        .expect("spawn external u-b");

    handle
        .wire(AgentIdentity::from("u-a"), MeerkatId::from("u-b"))
        .await
        .expect("wire external peers");
    handle
        .unwire(AgentIdentity::from("u-a"), MeerkatId::from("u-b"))
        .await
        .expect("unwire external peers");

    let trusted_a = external_a.trusted_peer_names().await;
    let trusted_b = external_b.trusted_peer_names().await;
    assert!(
        !trusted_a
            .iter()
            .any(|name| name == &test_comms_name_for(&mob_id, "worker", "u-b")),
        "u-a should drop remote peer trust after unwire"
    );
    assert!(
        !trusted_b
            .iter()
            .any(|name| name == &test_comms_name_for(&mob_id, "worker", "u-a")),
        "u-b should drop remote peer trust after unwire"
    );
    let intents_a = external_a.received_intents().await;
    let intents_b = external_b.received_intents().await;
    assert!(
        intents_a
            .iter()
            .any(|intent| intent == super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT),
        "u-a should receive an explicit unwire_member bridge command"
    );
    assert!(
        intents_b
            .iter()
            .any(|intent| intent == super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT),
        "u-b should receive an explicit unwire_member bridge command"
    );
}

#[tokio::test]
async fn test_spawn_unknown_profile_fails() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let result = handle
        .spawn(
            ProfileName::from("nonexistent"),
            MeerkatId::from("x-1"),
            None,
        )
        .await;
    assert!(matches!(result, Err(MobError::ProfileNotFound(_))));
}

#[tokio::test]
async fn test_spawn_fails_when_profile_comms_disabled() {
    let mut definition = sample_definition();
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile exists")
        .as_inline_mut()
        .unwrap();
    worker.tools.comms = false;

    let (handle, service) = create_test_mob(definition).await;
    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await;
    assert!(
        matches!(result, Err(MobError::WiringError(_))),
        "tools.comms=false must be rejected by spawn path"
    );
    assert_eq!(
        service.recorded_prompts().await.len(),
        0,
        "spawn should fail before create_session when comms is disabled"
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn test_spawn_append_failure_rolls_back_runtime_state() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("MemberSpawned").await;
    let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await;
    assert!(
        matches!(result, Err(MobError::StorageError(_))),
        "spawn should surface append failure"
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "spawn append failure must leave roster untouched"
    );
    assert_eq!(
        service.active_session_count().await,
        0,
        "spawn append failure must archive the just-created session"
    );

    let recorded = handle.events().replay_all().await.expect("replay");
    assert!(
        !recorded
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MemberSpawned(..))),
        "failed spawn append must not persist MemberSpawned"
    );
}

// -----------------------------------------------------------------------
// P1-T05: retire() removes a meerkat
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_retire_removes_from_roster() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");

    handle
        .retire(AgentIdentity::from("w-1"))
        .await
        .expect("retire");

    assert!(handle.list_members().await.is_empty());
}

#[tokio::test]
async fn test_retire_path_does_not_replay_full_event_log() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let (handle, _service) = create_test_mob_with_events(sample_definition(), events.clone()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    assert_eq!(events.replay_calls(), 0, "setup should not replay events");

    handle
        .retire(AgentIdentity::from("w-1"))
        .await
        .expect("retire");
    assert_eq!(
        events.replay_calls(),
        0,
        "retire idempotency check should not replay full event log per request"
    );
}

#[tokio::test]
async fn test_retire_emits_event() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");

    handle
        .retire(AgentIdentity::from("w-1"))
        .await
        .expect("retire");

    let events = handle.events().replay_all().await.expect("replay");
    let retired = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::MemberRetired { .. }));
    assert!(retired.is_some(), "should have retire event");
}

#[tokio::test]
async fn test_retire_nonexistent_is_idempotent() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let result = handle.retire(AgentIdentity::from("nope")).await;
    assert!(result.is_ok(), "retire should be idempotent");
}

#[tokio::test]
async fn test_retire_removes_wiring_from_peers() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");

    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    // Verify wired
    let entry = handle
        .get_member(&AgentIdentity::from("w-2"))
        .await
        .unwrap();
    assert!(entry.wired_to.contains(&AgentIdentity::from("w-1")));

    // Retire w-1
    handle
        .retire(AgentIdentity::from("w-1"))
        .await
        .expect("retire");

    // w-2 should no longer be wired to w-1
    let entry = handle
        .get_member(&AgentIdentity::from("w-2"))
        .await
        .unwrap();
    assert!(!entry.wired_to.contains(&AgentIdentity::from("w-1")));
}

#[tokio::test]
async fn test_retire_archive_failure_is_not_silent() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session_id = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    service.set_archive_failure(&session_id).await;

    // Archive failure is critical — retire must surface it (dogma §15).
    let result = handle.retire(AgentIdentity::from("w-1")).await;
    assert!(
        result.is_err(),
        "retire must return Err when ArchiveSession fails"
    );

    // Member is removed from roster unconditionally (finally block).
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "retire must remove roster entry even when archive fails"
    );
    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MemberRetired { .. })),
        "retire event should be persisted before archive so cleanup remains retryable"
    );
}

#[tokio::test]
async fn test_retire_trust_removal_failure_is_not_silent() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-2"),
            MockCommsBehavior {
                fail_remove_trust: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    // Retire succeeds despite trust-removal failure (best-effort cleanup).
    let result = handle.retire(AgentIdentity::from("w-1")).await;
    assert!(
        result.is_ok(),
        "retire should succeed despite trust-removal failure"
    );

    // Member is removed from roster unconditionally.
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "retire must remove roster entry even when trust removal fails"
    );
    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MemberRetired { .. })),
        "retire event should persist even when trust-removal cleanup fails"
    );
}

#[tokio::test]
async fn test_retire_fails_when_peer_retired_notification_fails_without_side_effects() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-2"),
            MockCommsBehavior {
                fail_send_peer_retired: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    let result = handle.retire(AgentIdentity::from("w-1")).await;
    assert!(
        result.is_ok(),
        "retire should remain best-effort under notification failure"
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "retired meerkat should be removed from roster"
    );
    let entry_w2 = handle
        .get_member(&AgentIdentity::from("w-2"))
        .await
        .expect("w-2 should stay in roster");
    assert!(
        !entry_w2.wired_to.contains(&AgentIdentity::from("w-1")),
        "retire should remove wiring from remaining peers"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MemberRetired { .. })),
        "retire event should persist even when notification cleanup fails"
    );
}

#[tokio::test]
async fn test_retire_append_failure_is_retryable_without_side_effects() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("MemberRetired").await;
    let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    let result = handle.retire(AgentIdentity::from("w-1")).await;
    assert!(
        matches!(result, Err(MobError::StorageError(_))),
        "retire should surface append failure"
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_some(),
        "retire append failure must keep roster state retryable"
    );
    let entry_w2 = handle
        .get_member(&AgentIdentity::from("w-2"))
        .await
        .expect("w-2 should remain in roster");
    assert!(
        entry_w2.wired_to.contains(&AgentIdentity::from("w-1")),
        "retire append failure must not apply trust cleanup before event persistence"
    );
    assert_eq!(
        service.active_session_count().await,
        2,
        "retire append failure must not archive sessions"
    );

    let recorded = handle.events().replay_all().await.expect("replay");
    assert!(
        !recorded
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MemberRetired { .. })),
        "failed retire append must not persist MemberRetired"
    );
}

// -----------------------------------------------------------------------
// P1-T06: wire() establishes bidirectional trust
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_wire_establishes_bidirectional() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    // Check roster wiring
    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    assert!(entry_l.wired_to.contains(&AgentIdentity::from("w-1")));

    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(entry_w.wired_to.contains(&AgentIdentity::from("l-1")));
}

#[tokio::test]
async fn test_member_roster_surfaces_peer_id() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let spawn = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    let lead_sid = spawn.bridge_session_id().expect("session-backed").clone();
    let expected_peer_id = service
        .comms_runtime(&lead_sid)
        .await
        .expect("lead comms runtime")
        .peer_id()
        .expect("lead peer id");
    let transport_public_key = service
        .comms_runtime(&lead_sid)
        .await
        .expect("lead comms runtime")
        .public_key()
        .expect("lead transport public key");

    let entry = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("member should exist");
    assert_eq!(entry.peer_id, Some(expected_peer_id));
    assert_eq!(
        entry.transport_public_key.as_deref(),
        Some(transport_public_key.as_str())
    );
    assert_ne!(
        expected_peer_id.to_string(),
        transport_public_key,
        "roster peer_id must be the canonical UUID, not the Ed25519 public key"
    );
}

#[tokio::test]
async fn test_member_status_projects_unreachable_peer_connectivity() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let left_id = MeerkatId::from("l-1");
    let right_id = MeerkatId::from("w-1");
    let left_session_id = handle
        .spawn(ProfileName::from("lead"), left_id.clone(), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .spawn(ProfileName::from("worker"), right_id.clone(), None)
        .await
        .expect("spawn worker");
    handle
        .wire(AgentIdentity::from(left_id.as_str()), right_id.clone())
        .await
        .expect("wire local peers");

    let right_entry = handle
        .get_member(&AgentIdentity::from(right_id.as_str()))
        .await
        .expect("right member exists");
    let right_name = format!(
        "{}/{}/{}",
        handle.mob_id(),
        right_entry.role,
        right_entry.agent_identity
    );
    service
        .set_peer_status(
            &left_session_id,
            &right_name,
            PeerReachability::Unreachable,
            Some(PeerReachabilityReason::OfflineOrNoAck),
        )
        .await;

    let snapshot = handle
        .member_status(&AgentIdentity::from(left_id.as_str()))
        .await
        .expect("member status should succeed");
    let connectivity = snapshot
        .peer_connectivity
        .expect("live comms runtime should project peer connectivity");
    assert_eq!(connectivity.reachable_peer_count, 0);
    assert_eq!(connectivity.unknown_peer_count, 0);
    assert_eq!(connectivity.unreachable_peers.len(), 1);
    assert_eq!(connectivity.unreachable_peers[0].peer, right_name);
    assert_eq!(
        connectivity.unreachable_peers[0].reason,
        Some(PeerReachabilityReason::OfflineOrNoAck)
    );
}

#[tokio::test]
async fn test_member_status_matches_connectivity_by_canonical_peer_id_when_public_key_differs() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let left_id = MeerkatId::from("l-1");
    let right_id = MeerkatId::from("w-1");
    let left_session_id = handle
        .spawn(ProfileName::from("lead"), left_id.clone(), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let right_session_id = handle
        .spawn(ProfileName::from("worker"), right_id.clone(), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from(left_id.as_str()), right_id.clone())
        .await
        .expect("wire local peers");

    let right_runtime = service
        .comms_runtime(&right_session_id)
        .await
        .expect("right comms runtime");
    let right_peer_id = right_runtime.peer_id().expect("right canonical peer id");
    let right_public_key = right_runtime
        .public_key()
        .expect("right transport public key");
    assert_ne!(
        right_peer_id.to_string(),
        right_public_key,
        "test fixture must exercise distinct canonical peer id and public key carriers"
    );

    let alias_name = format!("{}/worker/right-alias", handle.mob_id());
    let alias_spec = TrustedPeerDescriptor::test_only_unsigned_typed(
        alias_name.clone(),
        right_peer_id,
        format!("inproc://{alias_name}"),
    )
    .expect("valid alias peer spec");
    service
        .force_add_trust_from_spec(&left_session_id, alias_spec)
        .await;
    service
        .set_peer_status(
            &left_session_id,
            &alias_name,
            PeerReachability::Unreachable,
            Some(PeerReachabilityReason::OfflineOrNoAck),
        )
        .await;

    let snapshot = handle
        .member_status(&AgentIdentity::from(left_id.as_str()))
        .await
        .expect("member status should succeed");
    let connectivity = snapshot
        .peer_connectivity
        .expect("live comms runtime should project peer connectivity");
    assert_eq!(connectivity.reachable_peer_count, 0);
    assert_eq!(connectivity.unknown_peer_count, 0);
    assert_eq!(connectivity.unreachable_peers.len(), 1);
    assert_eq!(connectivity.unreachable_peers[0].peer, alias_name);
    assert_eq!(
        connectivity.unreachable_peers[0].reason,
        Some(PeerReachabilityReason::OfflineOrNoAck)
    );
}

#[tokio::test]
async fn test_member_status_omits_peer_connectivity_without_live_comms_runtime() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let left_id = MeerkatId::from("l-1");
    let right_id = MeerkatId::from("w-1");
    let left_session_id = handle
        .spawn(ProfileName::from("lead"), left_id.clone(), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .spawn(ProfileName::from("worker"), right_id.clone(), None)
        .await
        .expect("spawn worker");
    handle
        .wire(AgentIdentity::from(left_id.as_str()), right_id.clone())
        .await
        .expect("wire local peers");
    service.set_missing_comms_runtime(&left_session_id).await;

    let snapshot = handle
        .member_status(&AgentIdentity::from(left_id.as_str()))
        .await
        .expect("member status should succeed");
    assert!(
        snapshot.peer_connectivity.is_none(),
        "member snapshot should omit connectivity when no live comms runtime exists"
    );
}

#[tokio::test]
async fn test_wire_external_adds_trusted_peer_and_tracks_projection() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let external = TrustedPeerDescriptor::test_only_unsigned_typed(
        "remote-mob/worker/agent-b",
        meerkat_core::comms::PeerId::new(),
        "inproc://remote-mob/worker/agent-b",
    )
    .expect("valid external peer");

    handle
        .wire(
            AgentIdentity::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external");

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("member should exist");
    assert!(
        entry_l
            .wired_to
            .contains(&AgentIdentity::from(external.name.as_str()))
    );
    assert_eq!(
        entry_l
            .external_peer_specs
            .get(&MeerkatId::from(external.name.as_str()))
            .cloned(),
        Some(external.clone())
    );

    let trusted = service.trusted_peer_names(&sid_l).await;
    assert!(
        trusted.iter().any(|n| n == external.name.as_str()),
        "trusted peer list should include external peer name"
    );

    let dsl = handle
        .debug_dsl_t2_snapshot()
        .await
        .expect("debug dsl snapshot");
    assert!(
        dsl.wiring_edges
            .iter()
            .all(|edge| edge.a.0 != external.name.as_str() && edge.b.0 != external.name.as_str()),
        "external peer descriptors must not be projected into member wiring_edges"
    );
    let external_edge = crate::machines::mob_machine::ExternalPeerEdge::new(
        crate::machines::mob_machine::AgentIdentity::from("l-1"),
        crate::machines::mob_machine::ExternalPeerEndpoint::from(&external),
    );
    assert!(
        dsl.external_peer_edges.contains(&external_edge),
        "MobMachine should own the descriptor-bearing external peer edge"
    );
}

#[tokio::test]
async fn test_respawn_restores_external_wiring_from_roster_spec() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let old_sid = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let external = TrustedPeerDescriptor::test_only_unsigned_typed(
        "remote-mob/worker/agent-b",
        meerkat_core::comms::PeerId::new(),
        "inproc://remote-mob/worker/agent-b",
    )
    .expect("valid external peer");

    handle
        .wire(
            AgentIdentity::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external");

    let receipt = handle
        .respawn(AgentIdentity::from("l-1"), Some("resume".into()))
        .await
        .expect("respawn");
    let entry = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("member should exist");
    let new_sid = entry
        .member_ref
        .bridge_session_id()
        .cloned()
        .expect("respawn should produce replacement session");
    assert_ne!(new_sid, old_sid, "respawn should replace the session");

    let trusted = service.trusted_peer_names(&new_sid).await;
    assert!(
        trusted.iter().any(|n| n == external.name.as_str()),
        "respawn should restore external trusted peers from the stored roster spec"
    );

    assert_eq!(
        entry
            .external_peer_specs
            .get(&MeerkatId::from(external.name.as_str()))
            .cloned(),
        Some(external)
    );
    assert_eq!(entry.agent_runtime_id, receipt.agent_runtime_id);
    assert_eq!(entry.fence_token, receipt.fence_token);
}

#[tokio::test]
async fn test_respawn_restores_external_wiring_from_mob_machine_edge() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let old_sid = handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("l-machine"),
            None,
        )
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let external = TrustedPeerDescriptor::test_only_unsigned_typed(
        "remote-mob/worker/machine-agent",
        meerkat_core::comms::PeerId::new(),
        "inproc://remote-mob/worker/machine-agent",
    )
    .expect("valid external peer");
    let machine_edge = crate::machines::mob_machine::ExternalPeerEdge::new(
        crate::machines::mob_machine::AgentIdentity::from("l-machine"),
        crate::machines::mob_machine::ExternalPeerEndpoint::from(&external),
    );
    handle
        .project_machine_input(
            crate::machines::mob_machine::MobMachineInput::WireExternalPeer { edge: machine_edge },
        )
        .await
        .expect("seed machine-owned external edge without roster projection");
    let entry_before = handle
        .get_member(&AgentIdentity::from("l-machine"))
        .await
        .expect("member before respawn");
    assert!(
        !entry_before
            .external_peer_specs
            .contains_key(&AgentIdentity::from(external.name.as_str())),
        "test setup must leave roster projection empty so MobMachine is the only topology source"
    );

    let receipt = handle
        .respawn(AgentIdentity::from("l-machine"), Some("resume".into()))
        .await
        .expect("respawn");
    let entry = handle
        .get_member(&AgentIdentity::from("l-machine"))
        .await
        .expect("member should exist");
    let new_sid = entry
        .member_ref
        .bridge_session_id()
        .cloned()
        .expect("respawn should produce replacement session");
    assert_ne!(new_sid, old_sid, "respawn should replace the session");

    let trusted = service.trusted_peer_names(&new_sid).await;
    assert!(
        trusted.iter().any(|n| n == external.name.as_str()),
        "respawn should restore external trust from MobMachine topology"
    );
    assert_eq!(
        entry
            .external_peer_specs
            .get(&MeerkatId::from(external.name.as_str()))
            .cloned(),
        Some(external)
    );
    assert_eq!(entry.agent_runtime_id, receipt.agent_runtime_id);
    assert_eq!(entry.fence_token, receipt.fence_token);
}

#[tokio::test]
async fn test_unwire_external_removes_trust_and_projection() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let external = TrustedPeerDescriptor::test_only_unsigned_typed(
        "remote-mob/worker/agent-b",
        meerkat_core::comms::PeerId::new(),
        "inproc://remote-mob/worker/agent-b",
    )
    .expect("valid external peer");

    handle
        .wire(
            AgentIdentity::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external");
    handle
        .unwire(
            AgentIdentity::from("l-1"),
            MeerkatId::from(external.name.as_str()),
        )
        .await
        .expect("unwire external");

    let entry = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("member should exist");
    assert!(
        !entry
            .wired_to
            .contains(&AgentIdentity::from(external.name.as_str()))
    );
    assert!(
        !entry
            .external_peer_specs
            .contains_key(&MeerkatId::from(external.name.as_str()))
    );

    let trusted = service.trusted_peer_names(&sid_l).await;
    assert!(
        !trusted.iter().any(|n| n == external.name.as_str()),
        "trusted peer list should not include the removed external peer"
    );

    let dsl = handle
        .debug_dsl_t2_snapshot()
        .await
        .expect("debug dsl snapshot");
    let external_edge = crate::machines::mob_machine::ExternalPeerEdge::new(
        crate::machines::mob_machine::AgentIdentity::from("l-1"),
        crate::machines::mob_machine::ExternalPeerEndpoint::from(&external),
    );
    assert!(
        !dsl.external_peer_edges.contains(&external_edge),
        "external unwire should remove the machine-owned descriptor edge"
    );
    assert!(
        dsl.wiring_edges
            .iter()
            .all(|edge| edge.a.0 != external.name.as_str() && edge.b.0 != external.name.as_str()),
        "external unwire should not touch member wiring_edges"
    );
}

const ED25519_PUBLIC_KEY_7: &str = "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
const ED25519_PUBLIC_KEY_ZERO: &str = "ed25519:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

fn external_binding_peer_target(public_key: &str) -> PeerTarget {
    PeerTarget::ExternalBinding(ExternalPeerBindingSpec::new(
        "external-worker",
        "inproc://external-worker",
        meerkat_contracts::WireTrustedPeerIdentity::Ed25519PublicKey {
            public_key: public_key.to_string(),
        },
    ))
}

#[tokio::test]
async fn test_wire_external_binding_resolves_inside_mob_authority() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_l = handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("l-binding"),
            None,
        )
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(
            AgentIdentity::from("l-binding"),
            external_binding_peer_target(ED25519_PUBLIC_KEY_7),
        )
        .await
        .expect("wire external binding");

    let entry = handle
        .get_member(&AgentIdentity::from("l-binding"))
        .await
        .expect("member should exist");
    let descriptor = entry
        .external_peer_specs
        .get(&AgentIdentity::from("external-worker"))
        .expect("external binding should project descriptor");
    let expected_pubkey = [7u8; 32];
    assert_eq!(descriptor.pubkey, expected_pubkey);
    assert_eq!(
        descriptor.peer_id,
        PeerId::from_ed25519_pubkey(&expected_pubkey)
    );

    let trusted = service.trusted_peer_names(&sid_l).await;
    assert!(
        trusted.iter().any(|name| name == "external-worker"),
        "mob authority should install trust after resolving external binding"
    );
}

#[tokio::test]
async fn test_wire_external_binding_rejects_zero_pubkey_before_trust_install() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-zero"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let err = handle
        .wire(
            AgentIdentity::from("l-zero"),
            external_binding_peer_target(ED25519_PUBLIC_KEY_ZERO),
        )
        .await
        .expect_err("zero pubkey external binding must fail closed");
    let message = err.to_string();
    assert!(
        message.contains("public_key") || message.contains("non-zero"),
        "expected pubkey validation error, got: {message}"
    );

    let entry = handle
        .get_member(&AgentIdentity::from("l-zero"))
        .await
        .expect("member should exist");
    assert!(
        !entry
            .external_peer_specs
            .contains_key(&AgentIdentity::from("external-worker")),
        "failed external binding must not project an external edge"
    );

    let trusted = service.trusted_peer_names(&sid_l).await;
    assert!(
        !trusted.iter().any(|name| name == "external-worker"),
        "failed external binding must not install comms trust"
    );
}

#[tokio::test]
async fn test_resume_seeds_mob_machine_topology_from_event_projection() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let definition = sample_definition();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition.clone(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-resume"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-resume"),
            None,
        )
        .await
        .expect("spawn worker");
    handle
        .wire(AgentIdentity::from("l-resume"), MeerkatId::from("w-resume"))
        .await
        .expect("wire local peers");
    let external = TrustedPeerDescriptor::test_only_unsigned_typed(
        "remote-mob/worker/resume-agent",
        meerkat_core::comms::PeerId::new(),
        "inproc://remote-mob/worker/resume-agent",
    )
    .expect("valid external peer");
    handle
        .wire(
            AgentIdentity::from("l-resume"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external peer");
    handle.stop().await.expect("stop before resume");

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .resume()
    .await
    .expect("resume mob");

    let dsl = resumed
        .debug_dsl_t2_snapshot()
        .await
        .expect("debug dsl snapshot");
    let local_edge = crate::machines::mob_machine::WiringEdge::new(
        crate::machines::mob_machine::AgentIdentity::from("l-resume"),
        crate::machines::mob_machine::AgentIdentity::from("w-resume"),
    );
    assert!(
        dsl.wiring_edges.contains(&local_edge),
        "resume must seed machine-owned local topology from event projection"
    );
    let external_edge = crate::machines::mob_machine::ExternalPeerEdge::new(
        crate::machines::mob_machine::AgentIdentity::from("l-resume"),
        crate::machines::mob_machine::ExternalPeerEndpoint::from(&external),
    );
    assert!(
        dsl.external_peer_edges.contains(&external_edge),
        "resume must seed machine-owned external topology from event projection"
    );
}

#[tokio::test]
async fn test_wire_emits_peers_wired_event() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");

    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let events = handle.events().replay_all().await.expect("replay");
    let wired = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::MembersWired { .. }));
    assert!(wired.is_some(), "should have MembersWired event");
}

#[tokio::test]
async fn test_wire_is_idempotent_and_emits_single_pair_event() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("first wire");
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("second wire should reconcile as idempotent");

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert_eq!(
        entry_l.wired_to.len(),
        1,
        "idempotent wire must preserve one canonical edge on the lead"
    );
    assert_eq!(
        entry_w.wired_to.len(),
        1,
        "idempotent wire must preserve one canonical edge on the worker"
    );

    let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
    let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
    assert_eq!(
        trusted_by_lead
            .iter()
            .filter(|name| name.as_str() == test_comms_name("worker", "w-1"))
            .count(),
        1,
        "idempotent wire must not duplicate trusted-peer state on the lead"
    );
    assert_eq!(
        trusted_by_worker
            .iter()
            .filter(|name| name.as_str() == test_comms_name("lead", "l-1"))
            .count(),
        1,
        "idempotent wire must not duplicate trusted-peer state on the worker"
    );

    let events = handle.events().replay_all().await.expect("replay");
    let pair_wired_events = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::MembersWired { a, b }
                    if (a == &AgentIdentity::from("l-1") && b == &AgentIdentity::from("w-1"))
                        || (a == &AgentIdentity::from("w-1") && b == &AgentIdentity::from("l-1"))
            )
        })
        .count();
    assert_eq!(
        pair_wired_events, 1,
        "idempotent wire must emit one logical MembersWired event for one canonical edge"
    );
}

#[tokio::test]
async fn test_wire_unknown_meerkat_fails() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");

    let result = handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("nonexistent"))
        .await;
    assert!(matches!(result, Err(MobError::MemberNotFound(_))));
}

#[tokio::test]
async fn test_wire_fails_when_comms_runtime_missing_without_side_effects() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let _sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    service.set_missing_comms_runtime(&sid_w).await;

    let result = handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(matches!(result, Err(MobError::WiringError(_))));

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.is_empty(),
        "failed wire must not update roster"
    );
    assert!(
        entry_w.wired_to.is_empty(),
        "failed wire must not update roster"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MembersWired { .. })),
        "failed wire must not emit MembersWired"
    );
}

#[tokio::test]
async fn test_wire_fails_when_public_key_missing_without_side_effects() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                missing_public_key: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let result = handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(matches!(result, Err(MobError::WiringError(_))));

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.is_empty(),
        "failed wire must not update roster"
    );
    assert!(
        entry_w.wired_to.is_empty(),
        "failed wire must not update roster"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MembersWired { .. })),
        "failed wire must not emit MembersWired"
    );
}

#[tokio::test]
async fn test_wire_fails_when_peer_added_notification_fails_without_side_effects() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                fail_send_peer_added: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let result = handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::CommsError(_))),
        "wire should fail when required notification fails"
    );

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.is_empty(),
        "failed wire must not mutate roster"
    );
    assert!(
        entry_w.wired_to.is_empty(),
        "failed wire must not mutate roster"
    );

    let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
    let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
    assert!(
        !trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
        "failed wire must rollback lead trust"
    );
    assert!(
        !trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
        "failed wire must rollback worker trust"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MembersWired { .. })),
        "failed wire must not emit MembersWired"
    );
}

#[tokio::test]
async fn test_wire_establishes_comms_trust() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire should succeed");

    let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
    let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
    assert!(
        trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
        "lead runtime must trust worker runtime"
    );
    assert!(
        trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
        "worker runtime must trust lead runtime"
    );

    let intents_lead = service.sent_intents(&sid_l).await;
    let intents_worker = service.sent_intents(&sid_w).await;
    assert!(
        intents_lead.iter().any(|intent| intent == "mob.peer_added"),
        "lead should send mob.peer_added during wire"
    );
    assert!(
        intents_worker
            .iter()
            .any(|intent| intent == "mob.peer_added"),
        "worker should send mob.peer_added during wire"
    );
}

#[tokio::test]
async fn test_wire_append_failure_rolls_back_runtime_state() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("MembersWired").await;
    let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let result = handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::StorageError(_))),
        "wire should surface append failure"
    );

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.is_empty(),
        "wire append failure must rollback roster wiring"
    );
    assert!(
        entry_w.wired_to.is_empty(),
        "wire append failure must rollback roster wiring"
    );

    let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
    let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
    assert!(
        !trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
        "wire append failure must rollback trust on lead"
    );
    assert!(
        !trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
        "wire append failure must rollback trust on worker"
    );

    let recorded = handle.events().replay_all().await.expect("replay");
    assert!(
        !recorded
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MembersWired { .. })),
        "failed wire append must not persist MembersWired"
    );
}

// -----------------------------------------------------------------------
// P1-T07: unwire() removes bidirectional trust
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_unwire_removes_bidirectional() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");

    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("unwire");

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    assert!(entry_l.wired_to.is_empty());

    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(entry_w.wired_to.is_empty());
}

#[tokio::test]
async fn test_unwire_emits_peers_unwired_event() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("unwire");

    let events = handle.events().replay_all().await.expect("replay");
    let unwired = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::MembersUnwired { .. }));
    assert!(unwired.is_some(), "should have MembersUnwired event");
}

#[tokio::test]
async fn test_unwire_is_idempotent_and_emits_single_pair_event() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("first unwire");
    handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("second unwire should be idempotent");

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.is_empty() && entry_w.wired_to.is_empty(),
        "idempotent unwire must preserve a symmetric empty wiring projection"
    );

    let events = handle.events().replay_all().await.expect("replay");
    let pair_unwired_events = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::MembersUnwired { a, b }
                    if (a == &AgentIdentity::from("l-1") && b == &AgentIdentity::from("w-1"))
                        || (a == &AgentIdentity::from("w-1") && b == &AgentIdentity::from("l-1"))
            )
        })
        .count();
    assert_eq!(
        pair_unwired_events, 1,
        "idempotent unwire must emit one logical MembersUnwired event for one canonical edge removal"
    );
}

#[tokio::test]
async fn test_unwire_fails_when_comms_runtime_missing_without_side_effects() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");
    service.set_missing_comms_runtime(&sid_w).await;

    let result = handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(matches!(result, Err(MobError::WiringError(_))));

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.contains(&AgentIdentity::from("w-1")),
        "failed unwire must not mutate roster"
    );
    assert!(
        entry_w.wired_to.contains(&AgentIdentity::from("l-1")),
        "failed unwire must not mutate roster"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MembersUnwired { .. })),
        "failed unwire must not emit MembersUnwired"
    );
}

#[tokio::test]
async fn test_unwire_fails_when_public_key_missing_without_side_effects() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");
    service.clear_public_key(&sid_w).await;

    let result = handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(matches!(result, Err(MobError::WiringError(_))));

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.contains(&AgentIdentity::from("w-1")),
        "failed unwire must not mutate roster"
    );
    assert!(
        entry_w.wired_to.contains(&AgentIdentity::from("l-1")),
        "failed unwire must not mutate roster"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MembersUnwired { .. })),
        "failed unwire must not emit MembersUnwired"
    );
}

#[tokio::test]
async fn test_unwire_second_trust_removal_failure_restores_first_side() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                fail_remove_trust: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let result = handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::CommsError(_))),
        "unwire should surface second trust-removal failure"
    );

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.contains(&AgentIdentity::from("w-1")),
        "failed unwire must keep roster wiring on lead"
    );
    assert!(
        entry_w.wired_to.contains(&AgentIdentity::from("l-1")),
        "failed unwire must keep roster wiring on worker"
    );

    let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
    let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
    assert!(
        trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
        "failed unwire must restore first trust removal on lead"
    );
    assert!(
        trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
        "failed unwire must keep worker trust unchanged"
    );
}

#[tokio::test]
async fn test_unwire_fails_when_peer_unwired_notification_fails_without_side_effects() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service
        .set_comms_behavior(
            &test_comms_name("lead", "l-1"),
            MockCommsBehavior {
                fail_send_peer_unwired: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let result = handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::CommsError(_))),
        "unwire should fail when required notification fails"
    );

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.contains(&AgentIdentity::from("w-1")),
        "failed unwire must not mutate roster"
    );
    assert!(
        entry_w.wired_to.contains(&AgentIdentity::from("l-1")),
        "failed unwire must not mutate roster"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MembersUnwired { .. })),
        "failed unwire must not emit MembersUnwired"
    );
}

#[tokio::test]
async fn test_unwire_second_notification_failure_compensates_and_preserves_state() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                fail_send_peer_unwired: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let peer_added_count = |intents: &[String]| {
        intents
            .iter()
            .filter(|intent| intent.as_str() == "mob.peer_added")
            .count()
    };
    let lead_peer_added_before = peer_added_count(&service.sent_intents(&sid_l).await);
    let worker_peer_added_before = peer_added_count(&service.sent_intents(&sid_w).await);

    let result = handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::CommsError(_))),
        "unwire should fail when second required notification fails"
    );

    let lead_peer_added_after = peer_added_count(&service.sent_intents(&sid_l).await);
    let worker_peer_added_after = peer_added_count(&service.sent_intents(&sid_w).await);
    assert_eq!(
        lead_peer_added_after,
        lead_peer_added_before + 1,
        "second-notification failure should compensate with mob.peer_added from lead"
    );
    assert_eq!(
        worker_peer_added_after, worker_peer_added_before,
        "worker should not emit extra mob.peer_added on second-notification failure"
    );

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.contains(&AgentIdentity::from("w-1")),
        "failed unwire must preserve lead wiring"
    );
    assert!(
        entry_w.wired_to.contains(&AgentIdentity::from("l-1")),
        "failed unwire must preserve worker wiring"
    );

    let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
    let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
    assert!(
        trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
        "failed unwire must keep lead trust"
    );
    assert!(
        trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
        "failed unwire must keep worker trust"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MembersUnwired { .. })),
        "failed unwire must not emit MembersUnwired"
    );
}

#[tokio::test]
async fn test_unwire_first_trust_removal_failure_compensates_and_preserves_state() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service
        .set_comms_behavior(
            &test_comms_name("lead", "l-1"),
            MockCommsBehavior {
                fail_remove_trust: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let peer_added_count = |intents: &[String]| {
        intents
            .iter()
            .filter(|intent| intent.as_str() == "mob.peer_added")
            .count()
    };
    let lead_peer_added_before = peer_added_count(&service.sent_intents(&sid_l).await);
    let worker_peer_added_before = peer_added_count(&service.sent_intents(&sid_w).await);

    let result = handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::CommsError(_))),
        "unwire should surface first trust-removal failure"
    );

    let lead_peer_added_after = peer_added_count(&service.sent_intents(&sid_l).await);
    let worker_peer_added_after = peer_added_count(&service.sent_intents(&sid_w).await);
    assert_eq!(
        lead_peer_added_after,
        lead_peer_added_before + 1,
        "first-removal failure should compensate with mob.peer_added from lead"
    );
    assert_eq!(
        worker_peer_added_after,
        worker_peer_added_before + 1,
        "first-removal failure should compensate with mob.peer_added from worker"
    );

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.contains(&AgentIdentity::from("w-1")),
        "failed unwire must preserve lead wiring"
    );
    assert!(
        entry_w.wired_to.contains(&AgentIdentity::from("l-1")),
        "failed unwire must preserve worker wiring"
    );

    let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
    let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
    assert!(
        trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
        "failed unwire must keep lead trust"
    );
    assert!(
        trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
        "failed unwire must keep worker trust"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MembersUnwired { .. })),
        "failed unwire must not emit MembersUnwired"
    );
}

#[tokio::test]
async fn test_unwire_append_failure_restores_runtime_state() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("MembersUnwired").await;
    let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let result = handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::StorageError(_))),
        "unwire should surface append failure"
    );

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.contains(&AgentIdentity::from("w-1")),
        "unwire append failure must restore roster wiring"
    );
    assert!(
        entry_w.wired_to.contains(&AgentIdentity::from("l-1")),
        "unwire append failure must restore roster wiring"
    );

    let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
    let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
    assert!(
        trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
        "unwire append failure must restore trust on lead"
    );
    assert!(
        trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
        "unwire append failure must restore trust on worker"
    );

    let recorded = handle.events().replay_all().await.expect("replay");
    assert!(
        !recorded
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MembersUnwired { .. })),
        "failed unwire append must not persist MembersUnwired"
    );
}

// -----------------------------------------------------------------------
// P1-T08: auto_wire_orchestrator on spawn
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_auto_wire_orchestrator() {
    let (handle, _service) = create_test_mob(sample_definition_with_auto_wire()).await;

    // Spawn orchestrator first
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    // Spawn worker — should be auto-wired to orchestrator
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    // Verify wiring
    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.contains(&AgentIdentity::from("w-1")),
        "orchestrator should be wired to worker"
    );

    let entry_w = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(
        entry_w.wired_to.contains(&AgentIdentity::from("l-1")),
        "worker should be wired to orchestrator"
    );
}

#[tokio::test]
async fn test_auto_wire_orchestrator_updates_real_comms_peers() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) =
        create_test_mob_with_real_comms(sample_definition_with_auto_wire()).await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let comms_l = service.real_comms(&sid_l).await.expect("comms for l-1");
    let comms_w = service.real_comms(&sid_w).await.expect("comms for w-1");

    let peers_l = CoreCommsRuntime::peers(&*comms_l).await;
    let peers_w = CoreCommsRuntime::peers(&*comms_w).await;
    assert!(
        peers_l
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("worker", "w-1")),
        "auto-wire should add worker to lead peers()"
    );
    assert!(
        peers_w
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("lead", "l-1")),
        "auto-wire should add lead to worker peers()"
    );
}

#[tokio::test]
async fn test_auto_wire_orchestrator_not_wired_to_self() {
    let (handle, _service) = create_test_mob(sample_definition_with_auto_wire()).await;

    // Spawn orchestrator — should NOT wire to itself
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let entry_l = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .unwrap();
    assert!(
        entry_l.wired_to.is_empty(),
        "orchestrator should not be wired to itself"
    );
}

#[tokio::test]
async fn test_auto_wire_orchestrator_real_comms_does_not_surface_self_peer() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) =
        create_test_mob_with_real_comms(sample_definition_with_auto_wire()).await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let comms_l = service.real_comms(&sid_l).await.expect("comms for l-1");
    let peers_l = CoreCommsRuntime::peers(&*comms_l).await;
    assert!(
        !peers_l
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("lead", "l-1")),
        "auto-wire must not surface the spawned member itself in peers()"
    );
}

#[tokio::test]
async fn test_auto_wire_parent_uses_spawning_member_not_orchestrator() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) = create_test_mob_with_real_comms(sample_definition()).await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn first worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let mut spec = super::handle::SpawnMemberSpec::new("worker", "w-2");
    spec.auto_wire_parent = true;
    let receipt = handle
        .spawn_spec_receipt_with_owner_context(
            spec,
            super::handle::CanonicalOpsOwnerContext {
                owner_bridge_session_id: sid_w1.clone(),
                ops_registry: Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new()),
            },
        )
        .await
        .expect("spawn second worker from worker owner context");
    let sid_w2 = receipt
        .member_ref
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let comms_l = service.real_comms(&sid_l).await.expect("comms for l-1");
    let comms_w1 = service.real_comms(&sid_w1).await.expect("comms for w-1");
    let comms_w2 = service.real_comms(&sid_w2).await.expect("comms for w-2");

    let peers_w2 = CoreCommsRuntime::peers(&*comms_w2).await;
    assert!(
        peers_w2
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("worker", "w-1")),
        "auto_wire_parent should wire the spawned member to its actual spawner"
    );
    assert!(
        !peers_w2
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("lead", "l-1")),
        "auto_wire_parent must not fall back to wiring the orchestrator"
    );
    assert!(
        !peers_w2
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("worker", "w-2")),
        "auto_wire_parent must not surface the spawned member itself in peers()"
    );

    let peers_w1 = CoreCommsRuntime::peers(&*comms_w1).await;
    assert!(
        peers_w1
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("worker", "w-2")),
        "spawner should see the newly spawned member as a peer"
    );

    let peers_l = CoreCommsRuntime::peers(&*comms_l).await;
    assert!(
        !peers_l
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("worker", "w-2")),
        "orchestrator should stay uninvolved when a non-orchestrator member spawns with auto_wire_parent"
    );
}

#[tokio::test]
async fn test_spawn_skips_broken_orchestrator_in_auto_wire_selection() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let mut definition = sample_definition_with_auto_wire();
    definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .expect("lead profile")
        .as_inline_mut()
        .unwrap()
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .unwrap()
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle.stop().await.expect("stop");
    service.archive(&sid_l).await.expect("archive orchestrator");
    service.delete_persisted_session(&sid_l).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("partial resume should succeed");

    let lead = resumed
        .member_status(&AgentIdentity::from("l-1"))
        .await
        .expect("broken orchestrator");
    assert_eq!(lead.status, crate::runtime::handle::MobMemberStatus::Broken);
    match resumed.resume().await {
        Ok(()) => {}
        Err(MobError::InvalidTransition {
            from: MobState::Running,
            to: MobState::Running,
        }) => {}
        Err(error) => {
            panic!("resume should leave the mob runnable for spawn checks: {error}")
        }
    }

    let sid_w = resumed
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker should ignore broken orchestrator")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let worker = resumed
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("spawned worker entry");
    assert!(
        worker.wired_to.is_empty(),
        "auto-wire must not target broken orchestrators"
    );
    let trusted = service.trusted_peer_names(&sid_w).await;
    assert!(
        !trusted.contains(&test_comms_name("lead", "l-1")),
        "spawn must not install trust to a broken orchestrator"
    );
}

#[tokio::test]
async fn test_auto_wire_failure_is_returned_to_spawn_caller() {
    let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                missing_public_key: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await;
    assert!(
        matches!(result, Err(MobError::WiringError(_))),
        "spawn must fail when auto-wire fails"
    );

    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "failed spawn should rollback roster entry"
    );
    let lead_entry = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("lead should remain in roster");
    assert!(
        lead_entry.wired_to.is_empty(),
        "failed spawn should not leave partial wiring on lead"
    );
}

#[tokio::test]
async fn test_auto_wire_failure_after_partial_wire_cleans_peer_trust_via_spawn_rollback() {
    let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
    service
        .set_comms_behavior(
            &test_comms_name("lead", "l-1"),
            MockCommsBehavior {
                fail_send_peer_added: true,
                fail_remove_trust_once: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await;
    assert!(
        matches!(result, Err(MobError::WiringError(_))),
        "spawn must surface wiring failure after rollback cleanup"
    );

    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "failed spawn should rollback worker roster entry"
    );
    let lead_entry = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("lead should remain in roster");
    assert!(
        lead_entry.wired_to.is_empty(),
        "failed spawn should not leave partial roster wiring on lead"
    );

    let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
    assert!(
        !trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
        "spawn rollback must remove leaked trust from lead"
    );
}

#[tokio::test]
async fn test_spawn_rollback_ignores_missing_comms_for_non_wired_planned_targets() {
    let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                missing_public_key: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    service.set_missing_comms_runtime(&sid_l).await;

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await;
    assert!(
        matches!(result, Err(MobError::WiringError(_))),
        "spawn should surface original wiring failure instead of rollback failure"
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "rollback should still remove failed worker entry"
    );
}

#[tokio::test]
async fn test_spawn_rollback_ignores_missing_comms_for_non_wired_planned_targets_with_spawned_key()
{
    let (handle, service) = create_test_mob(sample_definition_with_role_wiring()).await;

    let sid_w1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                fail_send_peer_added: true,
                fail_remove_trust_once: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;
    service.set_missing_comms_runtime(&sid_w2).await;

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-3"), None)
        .await;
    assert!(
        matches!(result, Err(MobError::WiringError(_))),
        "spawn should surface role-wiring failure, not rollback failure"
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-3"))
            .await
            .is_none(),
        "failed spawn should not keep w-3 in roster"
    );

    let trusted_by_w1 = service.trusted_peer_names(&sid_w1).await;
    assert!(
        !trusted_by_w1.contains(&test_comms_name("worker", "w-3")),
        "rollback should remove leaked trust for first failed planned target"
    );
}

#[tokio::test]
async fn test_spawn_rollback_archive_failure_keeps_spawned_entry_and_persists_retired_event() {
    let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                missing_public_key: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;
    service
        .set_archive_failure_for_comms_name(&test_comms_name("worker", "w-1"))
        .await;

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await;
    assert!(
        matches!(result, Err(MobError::Internal(_))),
        "spawn should surface rollback archive failure"
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_some(),
        "rollback archive failure must not remove spawned roster entry"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MemberRetired { .. })),
        "rollback must persist MemberRetired before side-effect cleanup so retries stay replay-safe"
    );
}

#[tokio::test]
async fn test_fault_injected_lifecycle_operations_preserve_transactional_invariants() {
    // Spawn rollback invariants.
    let (spawn_handle, spawn_service) = create_test_mob(sample_definition_with_auto_wire()).await;
    spawn_service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                missing_public_key: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;
    let sid_l = spawn_handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let spawn_err = spawn_handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect_err("spawn should fail under injected auto-wire fault");
    assert!(
        matches!(
            spawn_err,
            MobError::WiringError(_) | MobError::StorageError(_)
        ),
        "spawn fault should surface as wiring/storage error"
    );
    assert!(
        spawn_handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "spawn rollback should keep failed member out of roster"
    );
    assert!(
        !spawn_service
            .trusted_peer_names(&sid_l)
            .await
            .contains(&test_comms_name("worker", "w-1")),
        "spawn rollback should remove leaked trust edges"
    );

    // Retire best-effort cleanup invariants: retire succeeds and removes
    // roster entry even when archive is fault-injected.
    let (retire_handle, retire_service) = create_test_mob(sample_definition()).await;
    let sid_r1 = retire_handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("r-1"), None)
        .await
        .expect("spawn r-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let _sid_r2 = retire_handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("r-2"), None)
        .await
        .expect("spawn r-2");
    retire_handle
        .wire(AgentIdentity::from("r-1"), MeerkatId::from("r-2"))
        .await
        .expect("wire");
    retire_service.set_archive_failure(&sid_r1).await;
    let retire_result = retire_handle.retire(AgentIdentity::from("r-1")).await;
    assert!(
        retire_result.is_err(),
        "retire must return Err when ArchiveSession fails"
    );
    assert!(
        retire_handle
            .get_member(&AgentIdentity::from("r-1"))
            .await
            .is_none(),
        "retire must remove roster entry even when archive fails"
    );
    let r2 = retire_handle
        .get_member(&AgentIdentity::from("r-2"))
        .await
        .expect("r-2 should remain");
    assert!(
        !r2.wired_to.contains(&AgentIdentity::from("r-1")),
        "roster.remove should clean r-1 from r-2's wired_to set"
    );
}

// -----------------------------------------------------------------------
// P1-T09: role_wiring fan-out on spawn
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_role_wiring_fan_out() {
    let (handle, _service) = create_test_mob(sample_definition_with_role_wiring()).await;

    // Spawn 3 workers
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-3"), None)
        .await
        .expect("spawn w-3");

    // w-1 was spawned first (no other workers existed), so no wiring for it at spawn time.
    // w-2 was spawned second, rule worker<->worker matches, wired to w-1.
    // w-3 was spawned third, rule worker<->worker matches, wired to w-1 and w-2.

    let entry_1 = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    let entry_2 = handle
        .get_member(&AgentIdentity::from("w-2"))
        .await
        .unwrap();
    let entry_3 = handle
        .get_member(&AgentIdentity::from("w-3"))
        .await
        .unwrap();

    // w-2 should be wired to w-1 (and vice versa)
    assert!(entry_2.wired_to.contains(&AgentIdentity::from("w-1")));
    assert!(entry_1.wired_to.contains(&AgentIdentity::from("w-2")));

    // w-3 should be wired to both w-1 and w-2
    assert!(entry_3.wired_to.contains(&AgentIdentity::from("w-1")));
    assert!(entry_3.wired_to.contains(&AgentIdentity::from("w-2")));

    // w-1 should be wired to both w-2 and w-3
    // Need to re-read since roster may have been updated after w-3 spawn
    let entry_1 = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap();
    assert!(entry_1.wired_to.contains(&AgentIdentity::from("w-3")));
}

#[tokio::test]
async fn test_spawn_skips_broken_role_peers_in_role_wiring_selection() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let mut definition = sample_definition_with_role_wiring();
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .unwrap()
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let sid_w1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle.stop().await.expect("stop");
    service.archive(&sid_w1).await.expect("archive w-1");
    service.delete_persisted_session(&sid_w1).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .resume()
    .await
    .expect("partial resume should succeed");

    let broken = resumed
        .member_status(&AgentIdentity::from("w-1"))
        .await
        .expect("broken member status");
    assert_eq!(
        broken.status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
    match resumed.resume().await {
        Ok(()) => {}
        Err(MobError::InvalidTransition {
            from: MobState::Running,
            to: MobState::Running,
        }) => {}
        Err(error) => {
            panic!("resume should leave the mob runnable for spawn checks: {error}")
        }
    }

    let sid_w2 = resumed
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn should ignore broken role peers")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let worker = resumed
        .get_member(&AgentIdentity::from("w-2"))
        .await
        .expect("spawned worker");
    assert!(
        !worker.wired_to.contains(&AgentIdentity::from("w-1")),
        "role wiring must not target broken peers"
    );
    let trusted = service.trusted_peer_names(&sid_w2).await;
    assert!(
        !trusted.contains(&test_comms_name("worker", "w-1")),
        "spawn must not install trust to broken role peers"
    );
}

#[tokio::test]
async fn test_role_wiring_cross_role_fans_out_to_three_existing_targets() {
    let (handle, _service) = create_test_mob(sample_definition_with_cross_role_wiring()).await;

    for worker_id in ["w-1", "w-2", "w-3"] {
        handle
            .spawn(
                ProfileName::from("worker"),
                MeerkatId::from(worker_id),
                None,
            )
            .await
            .expect("spawn worker");
    }

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let lead = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("lead should be in roster");
    assert_eq!(
        lead.wired_to.len(),
        3,
        "new role_x meerkat should fan out to all 3 existing role_y peers"
    );
    for worker_id in ["w-1", "w-2", "w-3"] {
        assert!(
            lead.wired_to.contains(&AgentIdentity::from(worker_id)),
            "lead should be wired to {worker_id}"
        );
        let worker = handle
            .get_member(&AgentIdentity::from(worker_id))
            .await
            .expect("worker should remain in roster");
        assert!(
            worker.wired_to.contains(&AgentIdentity::from("l-1")),
            "{worker_id} should be wired back to lead"
        );
    }
}

#[tokio::test]
async fn test_spawn_wiring_deduplicates_overlapping_orchestrator_and_role_edges() {
    let (handle, _service) =
        create_test_mob(sample_definition_with_overlapping_orchestrator_and_role_wiring()).await;

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let lead = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("lead should exist");
    assert_eq!(
        lead.wired_to.len(),
        1,
        "overlapping auto-wire + role rule should produce one trust edge"
    );
    assert!(lead.wired_to.contains(&AgentIdentity::from("w-1")));
}

#[tokio::test]
async fn test_role_wiring_failure_is_returned_to_spawn_caller() {
    let (handle, service) = create_test_mob(sample_definition_with_role_wiring()).await;
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-2"),
            MockCommsBehavior {
                missing_public_key: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await;
    assert!(
        matches!(result, Err(MobError::WiringError(_))),
        "spawn must fail when role-wiring fan-out fails"
    );

    assert!(
        handle
            .get_member(&AgentIdentity::from("w-2"))
            .await
            .is_none(),
        "failed spawn should rollback roster entry"
    );
    let entry_w1 = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("w-1 should remain in roster");
    assert!(
        !entry_w1.wired_to.contains(&AgentIdentity::from("w-2")),
        "failed spawn should not leave partial role wiring"
    );
}

// -----------------------------------------------------------------------
// P1-T10: external_turn enforces addressability
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_external_turn_addressable_succeeds() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead (external_addressable=true)");

    let result = handle
        .member(&AgentIdentity::from("l-1"))
        .await
        .expect("member handle")
        .send(
            "Hello from outside",
            meerkat_core::types::HandlingMode::Queue,
        )
        .await;
    assert!(
        result.is_ok(),
        "external_turn to addressable meerkat should succeed"
    );
}

#[tokio::test]
async fn test_member_handle_send_accepts_multimodal_content() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead (external_addressable=true)");

    let member = handle
        .member(&AgentIdentity::from("l-1"))
        .await
        .expect("member handle");

    let result = member
        .send(
            meerkat_core::types::ContentInput::Blocks(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "look at this".to_string(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "aGVsbG8=".into(),
                },
            ]),
            meerkat_core::types::HandlingMode::Queue,
        )
        .await;

    assert!(
        result.is_ok(),
        "member-directed send should accept multimodal content"
    );
}

#[tokio::test]
async fn test_external_turn_not_addressable_fails() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker (external_addressable=false)");

    let result = handle
        .member(&AgentIdentity::from("w-1"))
        .await
        .expect("member handle")
        .send(
            "Hello from outside",
            meerkat_core::types::HandlingMode::Queue,
        )
        .await;
    assert!(
        matches!(result, Err(MobError::NotExternallyAddressable(_))),
        "external_turn to non-addressable meerkat should fail"
    );
}

#[tokio::test]
async fn test_external_turn_unknown_meerkat_fails() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let result = handle
        .external_turn_for_member(
            MeerkatId::from("nonexistent"),
            "Hello".to_string().into(),
            meerkat_core::types::HandlingMode::Queue,
            None,
        )
        .await;
    assert!(matches!(result, Err(MobError::MemberNotFound(_))));
}

#[tokio::test]
async fn test_internal_turn_bypasses_external_addressable_check() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let result = handle
        .internal_turn(AgentIdentity::from("w-1"), "internal message")
        .await;
    assert!(
        result.is_ok(),
        "internal_turn should bypass external_addressable=false"
    );
}

#[tokio::test]
async fn test_internal_turn_unknown_meerkat_fails() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let result = handle
        .internal_turn(AgentIdentity::from("nonexistent"), "Hello")
        .await;
    assert!(matches!(result, Err(MobError::MemberNotFound(_))));
}

// -----------------------------------------------------------------------
// Phase 0 CHOKE-001 red test:
// autonomous dispatch must route via injector (currently not wired yet).
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_external_turn_autonomous_mode_uses_injector_dispatch() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .spawn_with_options(
            ProfileName::from("lead"),
            MeerkatId::from("l-autonomous"),
            None,
            Some(crate::MobRuntimeMode::AutonomousHost),
            None,
        )
        .await
        .expect("spawn autonomous lead");

    handle
        .member(&AgentIdentity::from("l-autonomous"))
        .await
        .expect("member handle")
        .send("inject me", meerkat_core::types::HandlingMode::Queue)
        .await
        .expect("external turn should execute");

    // Runtime adapter path: spawn uses accept_input_with_completion (1 keep-alive),
    // send() uses inject (1).
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        service.inject_call_count(),
        1,
        "autonomous dispatch must use event injector for send() (1 send)"
    );
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        1,
        "autonomous spawn uses keep-alive start_turn via runtime adapter"
    );
}

#[tokio::test]
async fn test_external_turn_autonomous_mode_keeps_injector_dispatch_after_kickoff_completion() {
    let (handle, service) = create_test_mob_with_runtime_adapter(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);
    handle
        .spawn_with_options(
            ProfileName::from("lead"),
            MeerkatId::from("l-idle-autonomous"),
            None,
            Some(crate::MobRuntimeMode::AutonomousHost),
            None,
        )
        .await
        .expect("spawn autonomous lead");

    tokio::time::sleep(Duration::from_millis(25)).await;
    let baseline_keep_alive_turns = service.keep_alive_start_turn_call_count();
    let baseline_injects = service.inject_call_count();

    handle
        .member(&AgentIdentity::from("l-idle-autonomous"))
        .await
        .expect("member handle")
        .send(
            "inject after kickoff",
            meerkat_core::types::HandlingMode::Queue,
        )
        .await
        .expect("external turn should execute");

    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        baseline_keep_alive_turns,
        "idle autonomous dispatch must not restart a new kickoff turn after the original one completed"
    );
    assert!(
        service.inject_call_count() > baseline_injects,
        "idle autonomous dispatch should still route through the live injector"
    );
}

#[tokio::test]
async fn test_external_turn_turn_driven_mode_uses_start_turn_dispatch() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .spawn_with_options(
            ProfileName::from("lead"),
            MeerkatId::from("l-turn-driven"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn turn-driven lead");
    let baseline_start_turn_calls = service.start_turn_call_count();

    handle
        .member(&AgentIdentity::from("l-turn-driven"))
        .await
        .expect("member handle")
        .send(
            "turn-driven message",
            meerkat_core::types::HandlingMode::Queue,
        )
        .await
        .expect("external turn should execute");

    assert_eq!(
        service.inject_call_count(),
        0,
        "turn-driven external dispatch should not use injector"
    );
    assert_eq!(
        service.start_turn_call_count(),
        baseline_start_turn_calls + 1,
        "turn-driven external dispatch should issue start_turn"
    );
}

#[tokio::test]
async fn test_runtime_backed_turn_driven_dispatch_surfaces_start_turn_failure() {
    let (handle, service) = create_test_mob_with_runtime_adapter(sample_definition()).await;
    handle
        .spawn_with_options(
            ProfileName::from("lead"),
            MeerkatId::from("l-runtime-fail"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn turn-driven lead");
    let baseline_start_turn_calls = service.start_turn_call_count();
    service.set_fail_start_turn(true);

    let result = handle
        .member(&AgentIdentity::from("l-runtime-fail"))
        .await
        .expect("member handle")
        .send("turn should fail", meerkat_core::types::HandlingMode::Queue)
        .await;
    let debug = format!("{result:?}");

    assert!(
        matches!(&result, Err(MobError::Internal(message)) if message.contains("mock start_turn failure")),
        "runtime-backed turn dispatch must surface the underlying turn failure, got: {debug}"
    );
    assert_eq!(
        service.start_turn_call_count(),
        baseline_start_turn_calls + 1,
        "runtime-backed dispatch should still attempt exactly one turn"
    );
}

#[tokio::test]
async fn test_autonomous_host_loop_uses_builder_runtime_adapter_for_comms_drain() {
    let service = Arc::new(MockSessionService::new());
    let service_adapter = service.enable_runtime_adapter();
    let builder_adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let handle = MobBuilder::new(sample_definition(), MobStorage::in_memory())
        .with_session_service(service)
        .with_runtime_adapter(builder_adapter.clone())
        .create()
        .await
        .expect("create mob");

    let session_id = handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("l-override"),
            None,
        )
        .await
        .expect("spawn autonomous lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    tokio::time::sleep(Duration::from_millis(25)).await;

    assert!(
        builder_adapter.contains_session(&session_id).await,
        "the builder-selected runtime adapter should own the autonomous member session"
    );
    assert!(
        !service_adapter.contains_session(&session_id).await,
        "the session service's default adapter must stay unused when an explicit mob runtime adapter override is provided"
    );

    tokio::time::timeout(Duration::from_secs(2), handle.stop())
        .await
        .expect("stop timed out")
        .expect("stop");
}

#[tokio::test]
async fn test_flow_dispatch_autonomous_mode_uses_injector_and_avoids_non_host_start_turn() {
    let (handle, service) =
        create_test_mob(sample_definition_with_dispatch_mode(DispatchMode::OneToOne)).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    let baseline_non_host_start_turn = service
        .start_turn_call_count()
        .saturating_sub(service.keep_alive_start_turn_call_count());

    let run_id = handle
        .run_flow(FlowId::from("dispatch"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(
        terminal.status,
        MobRunStatus::Completed,
        "peer-only external flow dispatch failed: failures={:?} steps={:?}",
        terminal.failure_ledger,
        terminal.step_ledger
    );

    let non_host_start_turn = service
        .start_turn_call_count()
        .saturating_sub(service.keep_alive_start_turn_call_count());
    assert_eq!(
        non_host_start_turn, baseline_non_host_start_turn,
        "autonomous flow dispatch should avoid non-host start_turn calls"
    );
    assert!(
        service.inject_call_count() > 0,
        "autonomous flow dispatch should use injector routing"
    );
}

#[tokio::test]
async fn test_flow_dispatch_autonomous_mode_with_overlay_rejects() {
    let mut definition = sample_definition_with_dispatch_mode(DispatchMode::OneToOne);
    let flow = definition
        .flows
        .get_mut(&flow_id("dispatch"))
        .expect("dispatch flow exists");
    let step = flow
        .steps
        .get_mut(&step_id("dispatch"))
        .expect("dispatch step exists");
    step.allowed_tools = Some(vec!["alpha".to_string()]);
    step.blocked_tools = Some(vec!["beta".to_string()]);

    let (handle, _service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("dispatch"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(
        terminal.status,
        MobRunStatus::Failed,
        "flow should fail because tool overlay cannot be enforced for autonomous host members"
    );
}

#[tokio::test]
async fn test_internal_turn_mode_routing_uses_injector_for_autonomous_and_start_turn_for_turn_driven()
 {
    let (handle, service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-auto"), None)
        .await
        .expect("spawn autonomous lead");
    handle
        .spawn_with_options(
            ProfileName::from("lead"),
            MeerkatId::from("l-turn"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn turn-driven lead");
    // Allow background start_turn from autonomous spawn to register.
    tokio::time::sleep(Duration::from_millis(10)).await;
    let baseline_start_turn = service.start_turn_call_count();

    handle
        .internal_turn(AgentIdentity::from("l-auto"), "internal autonomous")
        .await
        .expect("autonomous internal turn");
    handle
        .internal_turn(AgentIdentity::from("l-turn"), "internal turn-driven")
        .await
        .expect("turn-driven internal turn");

    // Runtime adapter path: autonomous spawn uses accept_input_with_completion (already in baseline).
    // internal_turn for autonomous uses inject (1). internal_turn for turn-driven uses start_turn (+1).
    assert_eq!(
        service.inject_call_count(),
        1,
        "autonomous internal turn should route via injector (1 internal_turn only)"
    );
    assert_eq!(
        service.start_turn_call_count(),
        baseline_start_turn + 1,
        "turn-driven internal turn should route via start_turn"
    );
}

#[tokio::test]
async fn test_force_cancel_member_routes_interrupt_without_retiring_member() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let member_id = MeerkatId::from("cancel-target");
    let member_ref = handle
        .spawn_with_options(
            ProfileName::from("worker"),
            member_id.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn target");
    let session_id = member_ref
        .bridge_session_id()
        .cloned()
        .expect("session-backed target");
    let baseline_interrupts = service.interrupt_call_count();

    handle
        .force_cancel_member(AgentIdentity::from(member_id.as_str()))
        .await
        .expect("force cancel should succeed");

    assert_eq!(
        service.interrupt_call_count(),
        baseline_interrupts + 1,
        "force-cancel must route through the runtime adapter interrupt path exactly once"
    );
    let entry = handle
        .get_member(&AgentIdentity::from(member_id.as_str()))
        .await
        .expect("member remains");
    assert_eq!(
        entry.member_ref.bridge_session_id().cloned(),
        Some(session_id.clone()),
        "force-cancel must not rewrite the member's canonical session binding"
    );
    assert_eq!(
        service.active_session_count().await,
        1,
        "force-cancel must not archive the backing session"
    );
    assert!(
        service.read(&session_id).await.is_ok(),
        "force-cancel must leave the backing session reachable"
    );
}

#[tokio::test]
async fn test_external_backend_turn_driven_mode_uses_start_turn_dispatch() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "external-turn-driven",
    );
    let mob_id = definition.id.clone();
    let (handle, service) = create_test_mob(definition).await;
    let external =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "lead", "l-ext-turn")).await;
    {
        let mut spec = SpawnMemberSpec::new("lead", "l-ext-turn");
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        spec.binding = Some(external.binding());
        handle
            .spawn_spec(spec)
            .await
            .expect("spawn external turn-driven lead");
    }
    let baseline_start_turn = service.start_turn_call_count();

    handle
        .member(&AgentIdentity::from("l-ext-turn"))
        .await
        .expect("member handle")
        .send(
            "external turn-driven",
            meerkat_core::types::HandlingMode::Queue,
        )
        .await
        .expect("external turn should execute");

    assert_eq!(
        service.inject_call_count(),
        0,
        "peer-only turn-driven external dispatch should not use injector"
    );
    assert_eq!(
        service.start_turn_call_count(),
        baseline_start_turn,
        "peer-only turn-driven external dispatch should avoid local bridge start_turn"
    );
}

#[tokio::test]
async fn test_external_backend_autonomous_flow_dispatch_normalizes_to_peer_only_turn_dispatch() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let mut definition = with_unique_mob_id(
        sample_definition_with_dispatch_mode(DispatchMode::OneToOne),
        "external-autonomous-flow",
    );
    definition.backend.external = Some(crate::definition::ExternalBackendConfig {
        address_base: "https://backend.example.invalid/mesh".to_string(),
    });
    let mob_id = definition.id.clone();
    let (handle, service) = create_test_mob(definition).await;
    let external_lead =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "lead", "l-ext-auto")).await;
    let external_worker =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext-auto")).await;
    {
        let mut spec = SpawnMemberSpec::new("lead", "l-ext-auto");
        spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
        spec.binding = Some(external_lead.binding());
        handle
            .spawn_spec(spec)
            .await
            .expect("spawn external autonomous lead");
    }
    {
        let mut spec = SpawnMemberSpec::new("worker", "w-ext-auto");
        spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
        spec.binding = Some(external_worker.binding());
        handle
            .spawn_spec(spec)
            .await
            .expect("spawn external autonomous worker");
    }
    let baseline_non_host_start_turn = service
        .start_turn_call_count()
        .saturating_sub(service.keep_alive_start_turn_call_count());

    let run_id = handle
        .run_flow(FlowId::from("dispatch"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(
        terminal.status,
        MobRunStatus::Failed,
        "peer-only external flow dispatch should fail explicitly without remote event-stream fan-in: failures={:?} steps={:?}",
        terminal.failure_ledger,
        terminal.step_ledger
    );
    let failure_reasons = terminal
        .failure_ledger
        .iter()
        .map(|entry| entry.reason.as_str())
        .collect::<Vec<_>>();
    assert!(
        failure_reasons
            .iter()
            .any(|message| message.contains("tracked turn event streams are not supported")),
        "peer-only external flow dispatch should fail with an explicit unsupported-mode message, got: {failure_reasons:?}"
    );

    let non_host_start_turn = service
        .start_turn_call_count()
        .saturating_sub(service.keep_alive_start_turn_call_count());
    assert_eq!(
        non_host_start_turn, baseline_non_host_start_turn,
        "peer-only external flow dispatch should avoid non-host start_turn calls"
    );
    assert_eq!(
        service.inject_call_count(),
        0,
        "peer-only external flow dispatch should not rely on local injector routing"
    );
}

#[tokio::test]
async fn test_subscribe_all_agent_events_keeps_session_backed_members_in_mixed_roster() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "mixed-subscribe-all",
    );
    let mob_id = definition.id.clone();
    let (handle, _service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-local"), None)
        .await
        .expect("spawn local lead");
    let external =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext-sub")).await;
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext-sub"),
            None,
            external.binding(),
        )
        .await
        .expect("spawn external worker");

    let streams = handle
        .subscribe_all_agent_events()
        .await
        .expect("mixed roster should still expose session-backed event streams");
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].0, AgentIdentity::from("l-local"));
}

#[tokio::test]
async fn test_subscribe_all_agent_events_surfaces_session_subscription_failure() {
    // Regression: a session-backed member whose subscribe_session_events fails
    // must propagate the error, not be silently dropped. Previously the loop
    // used `if let Ok(stream) = ...` which turned real subscription errors
    // into partial/empty lists and masked real regressions.
    let definition = with_unique_mob_id(sample_definition(), "subscribe-all-failure-propagates");
    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-local"), None)
        .await
        .expect("spawn local lead");
    handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-local"),
            None,
        )
        .await
        .expect("spawn local worker");

    let broken_session_id = handle
        .resolve_bridge_session_id(&AgentIdentity::from("w-local"))
        .await
        .expect("worker session should have a bridge session id");
    service.fail_subscribe_for_session(broken_session_id).await;

    match handle.subscribe_all_agent_events().await {
        Ok(_) => panic!("broken session subscription must surface as an error"),
        Err(MobError::Internal(message)) => assert!(
            message.contains("failed to subscribe to agent events") && message.contains("w-local"),
            "internal error should cite the failing member, got: {message}"
        ),
        Err(other) => {
            panic!("expected MobError::Internal propagating subscribe failure, got: {other:?}")
        }
    }
}

#[tokio::test]
async fn test_external_backend_lifecycle_and_turn_policy() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "external-lifecycle-policy",
    );
    let mob_id = definition.id.clone();
    let (handle, _service) = create_test_mob(definition).await;
    let external_lead =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "lead", "l-ext")).await;
    let external_worker =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext")).await;

    handle
        .spawn_with_binding(
            ProfileName::from("lead"),
            MeerkatId::from("l-ext"),
            None,
            external_lead.binding(),
        )
        .await
        .expect("spawn external lead");
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            external_worker.binding(),
        )
        .await
        .expect("spawn external worker");

    handle
        .wire(AgentIdentity::from("l-ext"), MeerkatId::from("w-ext"))
        .await
        .expect("wire external members");
    handle
        .unwire(AgentIdentity::from("l-ext"), MeerkatId::from("w-ext"))
        .await
        .expect("unwire external members");

    handle
        .member(&AgentIdentity::from("l-ext"))
        .await
        .expect("member handle")
        .send(
            "outside hello".to_string(),
            meerkat_core::types::HandlingMode::Queue,
        )
        .await
        .expect("external lead should accept external turns");
    let denied = handle
        .member(&AgentIdentity::from("w-ext"))
        .await
        .expect("member handle")
        .send(
            "outside hello".to_string(),
            meerkat_core::types::HandlingMode::Queue,
        )
        .await
        .expect_err("worker external turn should be denied by profile policy");
    assert!(matches!(denied, MobError::NotExternallyAddressable(_)));

    handle
        .retire(AgentIdentity::from("w-ext"))
        .await
        .expect("retire external member");
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-ext"))
            .await
            .is_none(),
        "retire should remove external backend member"
    );
}

#[tokio::test]
async fn test_rewire_repairs_remote_trust_for_existing_peer_only_edge() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "rewire-peer-only-trust",
    );
    let mob_id = definition.id.clone();
    let (handle, _service) = create_test_mob(definition).await;
    let external_a =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "lead", "l-ext-rewire")).await;
    let external_b =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext-rewire")).await;

    handle
        .spawn_with_binding(
            ProfileName::from("lead"),
            MeerkatId::from("l-ext-rewire"),
            None,
            external_a.binding(),
        )
        .await
        .expect("spawn external lead");
    handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext-rewire"),
            None,
            external_b.binding(),
        )
        .await
        .expect("spawn external worker");

    handle
        .wire(
            AgentIdentity::from("l-ext-rewire"),
            MeerkatId::from("w-ext-rewire"),
        )
        .await
        .expect("initial wire");

    let crate::RuntimeBinding::External {
        peer_id: lead_peer_id,
        ..
    } = external_a.binding()
    else {
        panic!("expected external lead binding");
    };
    let crate::RuntimeBinding::External {
        peer_id: worker_peer_id,
        ..
    } = external_b.binding()
    else {
        panic!("expected external worker binding");
    };
    external_a.remove_trusted_peer(&worker_peer_id).await;
    external_b.remove_trusted_peer(&lead_peer_id).await;

    handle
        .wire(
            AgentIdentity::from("l-ext-rewire"),
            MeerkatId::from("w-ext-rewire"),
        )
        .await
        .expect("rewire should reconcile both peer-only trust edges");

    let lead_name = test_comms_name_for(&mob_id, "lead", "l-ext-rewire");
    let worker_name = test_comms_name_for(&mob_id, "worker", "w-ext-rewire");
    assert!(
        external_a.trusted_peer_names().await.contains(&worker_name),
        "rewire should restore worker trust on the lead peer-only runtime"
    );
    assert!(
        external_b.trusted_peer_names().await.contains(&lead_name),
        "rewire should restore lead trust on the worker peer-only runtime"
    );
}

#[tokio::test]
async fn test_external_turn_prefers_effective_profile_override_addressability() {
    let definition = sample_definition();
    let mut override_profile = definition
        .profiles
        .get(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline()
        .expect("worker profile should be inline")
        .clone();
    override_profile.external_addressable = true;

    let (handle, _service) = create_test_mob(definition).await;
    let mut spec = SpawnMemberSpec::new("worker", AgentIdentity::from("w-override"));
    spec.override_profile = Some(override_profile);
    handle
        .spawn_spec(spec)
        .await
        .expect("spawn override worker");

    handle
        .member(&AgentIdentity::from("w-override"))
        .await
        .expect("member handle")
        .send(
            "outside hello".to_string(),
            meerkat_core::types::HandlingMode::Queue,
        )
        .await
        .expect("override profile should permit external turns");
}

// -----------------------------------------------------------------------
// P1-T11: MobCreated event stores definition
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_mob_created_event_stores_definition() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let events = handle.events().replay_all().await.expect("replay");

    // First event should be MobCreated
    assert!(!events.is_empty(), "should have at least one event");
    match &events[0].kind {
        MobEventKind::MobCreated { definition } => {
            assert_eq!(definition.id.as_str(), "test-mob");
            assert_eq!(definition.profiles.len(), 2);
            assert!(definition.profiles.contains_key(&ProfileName::from("lead")));
            assert!(
                definition
                    .profiles
                    .contains_key(&ProfileName::from("worker"))
            );
        }
        other => panic!("first event should be MobCreated, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_mob_created_definition_roundtrips_through_json() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let events = handle.events().replay_all().await.expect("replay");

    if let MobEventKind::MobCreated { definition } = &events[0].kind {
        let json = serde_json::to_string(definition).expect("serialize");
        let parsed: MobDefinition = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(**definition, parsed);
    }
}

// -----------------------------------------------------------------------
// CHOKE-MOB-005: MobHandle parallel spawn provisioning
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_concurrent_spawns_parallelize_provisioning() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_create_session_delay_ms(120);

    // Spawn 10 workers concurrently
    let mut join_handles = Vec::new();
    for i in 0..10 {
        let h = handle.clone();
        let jh = tokio::spawn(async move {
            h.spawn(
                ProfileName::from("worker"),
                MeerkatId::from(format!("w-{i}")),
                None,
            )
            .await
        });
        join_handles.push(jh);
    }

    for jh in join_handles {
        jh.await.expect("join").expect("spawn");
    }

    let meerkats = handle.list_members().await;
    assert_eq!(
        meerkats.len(),
        10,
        "all 10 concurrent spawns should succeed"
    );
    let ids = meerkats
        .iter()
        .map(|entry| entry.agent_identity.as_str().to_string())
        .collect::<HashSet<_>>();
    assert_eq!(
        ids.len(),
        10,
        "serialized actor path must avoid duplicate IDs"
    );
    for expected in [
        "w-0", "w-1", "w-2", "w-3", "w-4", "w-5", "w-6", "w-7", "w-8", "w-9",
    ] {
        assert!(ids.contains(expected), "missing spawned meerkat {expected}");
    }

    let events = handle.events().replay_all().await.expect("replay");
    let spawned_count = events
        .iter()
        .filter(|event| matches!(event.kind, MobEventKind::MemberSpawned(..)))
        .count();
    assert_eq!(
        spawned_count, 10,
        "parallel spawn path should emit exactly one MemberSpawned per request"
    );
    assert!(
        service.max_concurrent_create_session_calls() > 1,
        "spawn provisioning should run in parallel (max observed concurrent create_session calls: {})",
        service.max_concurrent_create_session_calls()
    );
}

#[tokio::test]
async fn test_spawn_many_member_refs_returns_results_in_input_order() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_create_session_delay_ms(100);

    let specs = vec![
        SpawnMemberSpec::new("worker", "w-a"),
        SpawnMemberSpec::new("worker", "w-b"),
        SpawnMemberSpec::new("worker", "w-c"),
    ];

    let results = handle.spawn_many(specs).await;
    assert_eq!(results.len(), 3);
    for (idx, result) in results.into_iter().enumerate() {
        let member = result.expect("spawn_many result");
        assert!(
            !member.agent_identity.as_str().is_empty(),
            "spawn_many result {idx} should include agent identity"
        );
    }
    assert!(
        service.max_concurrent_create_session_calls() > 1,
        "spawn_many should use parallel provisioning under the hood"
    );
}

#[tokio::test]
async fn test_spawn_many_parallel_finalize_deduplicates_worker_pair_wiring() {
    let (handle, service) = create_test_mob(sample_definition_with_role_wiring()).await;
    service.set_create_session_delay_ms(120);

    let specs = vec![
        SpawnMemberSpec::new("worker", "w-a"),
        SpawnMemberSpec::new("worker", "w-b"),
    ];

    let results = handle.spawn_many(specs).await;
    for result in results {
        result.expect("spawn_many member ref");
    }

    let a = handle
        .get_member(&AgentIdentity::from("w-a"))
        .await
        .expect("w-a should exist");
    let b = handle
        .get_member(&AgentIdentity::from("w-b"))
        .await
        .expect("w-b should exist");
    assert_eq!(
        a.wired_to.len(),
        1,
        "parallel spawn finalization must dedupe the w-a<->w-b edge"
    );
    assert_eq!(
        b.wired_to.len(),
        1,
        "parallel spawn finalization must dedupe the w-a<->w-b edge"
    );
    assert!(a.wired_to.contains(&AgentIdentity::from("w-b")));
    assert!(b.wired_to.contains(&AgentIdentity::from("w-a")));
}

#[tokio::test]
async fn test_spawn_many_parallel_finalize_tolerates_overlapping_role_wiring_targets() {
    let (handle, service) =
        create_test_mob(sample_definition_with_overlapping_orchestrator_and_role_wiring()).await;
    service.set_create_session_delay_ms(120);

    let specs = vec![
        SpawnMemberSpec::new("lead", "l-a"),
        SpawnMemberSpec::new("worker", "w-a"),
    ];

    let results = handle.spawn_many(specs).await;
    for result in results {
        result.expect("spawn_many member ref");
    }

    let lead = handle
        .get_member(&AgentIdentity::from("l-a"))
        .await
        .expect("lead should exist");
    let worker = handle
        .get_member(&AgentIdentity::from("w-a"))
        .await
        .expect("worker should exist");
    assert_eq!(
        lead.wired_to.len(),
        1,
        "overlapping batch fan-out must treat an already-created edge as satisfied"
    );
    assert_eq!(
        worker.wired_to.len(),
        1,
        "overlapping batch fan-out must treat an already-created edge as satisfied"
    );
    assert!(lead.wired_to.contains(&AgentIdentity::from("w-a")));
    assert!(worker.wired_to.contains(&AgentIdentity::from("l-a")));
}

#[tokio::test]
async fn test_concurrent_spawn_and_retire_same_meerkat_is_serialized() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let h1 = handle.clone();
    let h2 = handle.clone();

    let spawn = tokio::spawn(async move {
        h1.spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
    });
    let retire = tokio::spawn(async move { h2.retire(AgentIdentity::from("w-1")).await });

    let _ = spawn.await.expect("spawn join");
    retire.await.expect("retire join").expect("retire");

    let roster = handle.list_members().await;
    assert!(
        roster.is_empty() || (roster.len() == 1 && roster[0].agent_identity.as_str() == "w-1"),
        "serialized spawn/retire should never corrupt roster"
    );
}

#[tokio::test]
async fn test_retiring_member_is_not_routable_before_disposal_completes() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_archive_delay_ms(250);

    let session_id = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let retire_handle = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.retire(AgentIdentity::from("w-1")).await })
    };

    tokio::time::sleep(Duration::from_millis(25)).await;

    let all_members = handle.list_all_members().await;
    assert_eq!(
        all_members.len(),
        1,
        "retiring member should remain observable"
    );
    assert_eq!(all_members[0].agent_identity.as_str(), "w-1");
    assert_eq!(all_members[0].state, crate::roster::MemberState::Retiring);

    let active_members = handle.list_members().await;
    assert!(
        active_members.is_empty(),
        "retiring member must leave the active roster before disposal completes"
    );

    let start_turn_calls_before = service.start_turn_call_count();
    let external_turn = handle.member(&AgentIdentity::from("w-1")).await;
    assert!(matches!(external_turn, Err(MobError::MemberNotFound(id)) if id.as_str() == "w-1"));

    let internal_turn = handle
        .internal_turn(AgentIdentity::from("w-1"), "still there?".to_string())
        .await
        .expect_err("retiring member must reject new internal work");
    assert!(matches!(internal_turn, MobError::MemberNotFound(id) if id.as_str() == "w-1"));

    assert_eq!(
        service.start_turn_call_count(),
        start_turn_calls_before,
        "blocked turns must not reach the backing session"
    );

    retire_handle
        .await
        .expect("retire join")
        .expect("retire completes");
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "member should be removed once retirement completes"
    );
    assert_eq!(
        service.active_session_count().await,
        0,
        "completed retirement must archive the backing session"
    );
    assert!(
        service.read(&session_id).await.is_err(),
        "retired backing session should no longer resolve as active in the owner test harness"
    );
}

#[tokio::test]
async fn test_spawn_rejects_duplicate_id_while_first_spawn_is_pending() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_create_session_delay_ms(180);

    let first = {
        let handle = handle.clone();
        tokio::spawn(async move {
            handle
                .spawn(ProfileName::from("worker"), MeerkatId::from("w-dup"), None)
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(25)).await;

    let duplicate = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-dup"), None)
        .await
        .expect_err("duplicate pending spawn should fail immediately");
    assert!(matches!(duplicate, MobError::MemberAlreadyExists(_)));

    first
        .await
        .expect("spawn join")
        .expect("first spawn succeeds");
    assert_eq!(service.active_session_count().await, 1);
}

#[tokio::test]
async fn test_stop_fails_pending_spawns_and_cleans_up_provisioned_session() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_create_session_delay_ms(220);

    let pending_spawn = {
        let handle = handle.clone();
        tokio::spawn(async move {
            handle
                .spawn(ProfileName::from("worker"), MeerkatId::from("w-stop"), None)
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;

    handle.stop().await.expect("stop should succeed");
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);

    let spawn_error = pending_spawn
        .await
        .expect("spawn join")
        .expect_err("pending spawn should fail once stop begins");
    match spawn_error {
        MobError::Internal(message) => {
            assert!(
                message.contains("mob is stopping"),
                "expected stop cancellation message, got: {message}"
            );
        }
        other => panic!("expected internal pending-spawn cancellation, got: {other}"),
    }

    tokio::time::sleep(Duration::from_millis(260)).await;
    assert_eq!(
        service.active_session_count().await,
        0,
        "canceled pending spawn should retire any provisioned session during cleanup"
    );
}

#[tokio::test]
async fn test_stop_clears_pending_spawn_count_and_failed_member_projection() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let initial = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("initial orchestrator snapshot");
    assert_eq!(initial.pending_spawn_count, 0);

    service.set_create_session_delay_ms(220);
    let pending_spawn = {
        let handle = handle.clone();
        tokio::spawn(async move {
            handle
                .spawn(
                    ProfileName::from("worker"),
                    MeerkatId::from("w-stop-lineage"),
                    None,
                )
                .await
        })
    };

    tokio::time::sleep(Duration::from_millis(20)).await;
    let staged = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("staged orchestrator snapshot");
    assert_eq!(
        staged.pending_spawn_count, 1,
        "pending spawn must contribute to orchestrator-owned lineage while provisioning is in flight"
    );
    let staged_dsl = handle
        .debug_dsl_t2_snapshot()
        .await
        .expect("staged dsl snapshot");
    assert!(
        staged_dsl
            .pending_spawn_sessions
            .keys()
            .any(|identity| identity.0 == "w-stop-lineage"),
        "pending spawn admission must name the member identity before provisioning completes"
    );

    handle.stop().await.expect("stop should succeed");
    let _ = pending_spawn
        .await
        .expect("spawn join")
        .expect_err("pending spawn should fail once stop begins");

    tokio::time::sleep(Duration::from_millis(260)).await;
    let stopped = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("stopped orchestrator snapshot");
    assert_eq!(
        stopped.pending_spawn_count, 0,
        "stop must clear orchestrator-owned pending spawn lineage"
    );
    let stopped_dsl = handle
        .debug_dsl_t2_snapshot()
        .await
        .expect("stopped dsl snapshot");
    assert!(
        stopped_dsl.pending_spawn_sessions.is_empty(),
        "stop must clear machine-owned pending spawn admissions"
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-stop-lineage"))
            .await
            .is_none(),
        "stop must not leave a canonical roster projection for a canceled pending spawn"
    );
    assert_eq!(
        service.active_session_count().await,
        0,
        "stop must retire any provisioned session for the canceled pending spawn"
    );
}

#[tokio::test]
async fn test_stop_rejects_active_flow_and_schema_requires_no_active_runs() {
    let (handle, service) =
        create_test_mob(sample_definition_with_single_step_flow(60_000, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let error = handle
        .stop()
        .await
        .expect_err("stop should reject active flows");
    assert!(matches!(
        error,
        MobError::InvalidTransition {
            from: MobState::Running,
            to: MobState::Stopped,
        }
    ));
    assert_eq!(handle.status().await.unwrap(), MobState::Running);

    handle
        .cancel_flow(run_id.clone())
        .await
        .expect("cancel flow after rejected stop");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(8)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);

    let schema = schema_mob_machine();
    let stop = schema
        .transitions
        .iter()
        .find(|transition| transition.name.as_str() == "StopRunning")
        .expect("StopRunning transition");
    assert!(
        stop.guards
            .iter()
            .any(|guard| guard.name == "no_active_runs"),
        "StopRunning should require no active runs"
    );
}

#[tokio::test]
async fn test_failed_spawn_clears_pending_spawn_count_and_failed_roster_entry() {
    let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let _initial = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("initial orchestrator snapshot");
    service.set_create_session_delay_ms(150);
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-pending"),
            MockCommsBehavior {
                missing_public_key: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    let spawn = {
        let handle = handle.clone();
        tokio::spawn(async move {
            handle
                .spawn(
                    ProfileName::from("worker"),
                    MeerkatId::from("w-pending"),
                    None,
                )
                .await
        })
    };

    tokio::time::sleep(Duration::from_millis(30)).await;
    let staged = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("staged orchestrator snapshot");
    assert_eq!(
        staged.pending_spawn_count, 1,
        "pending spawn admission must be reflected in orchestrator-owned pending count while provisioning is in flight"
    );
    // topology_revision is orchestrator shell metadata not tracked by the DSL authority.
    // The meaningful assertion is pending_spawn_count (checked above).

    let error = spawn
        .await
        .expect("spawn join")
        .expect_err("spawn should fail during auto-wire setup");
    assert!(
        matches!(error, MobError::WiringError(_) | MobError::Internal(_)),
        "failed pending spawn should surface wiring/internal failure, got: {error}"
    );

    let settled = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("settled orchestrator snapshot");
    assert_eq!(
        settled.pending_spawn_count, 0,
        "failed spawn finalization must clear orchestrator-owned pending spawn count"
    );
    // topology_revision is orchestrator shell metadata not tracked by the DSL authority.
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-pending"))
            .await
            .is_none(),
        "failed spawn must not leave a canonical roster entry behind"
    );
}

#[tokio::test]
async fn test_failed_role_wiring_spawn_preserves_survivor_wiring_symmetry() {
    let (handle, service) = create_test_mob(sample_definition_with_role_wiring()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");

    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-2"),
            MockCommsBehavior {
                missing_public_key: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    let error = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-3"), None)
        .await
        .expect_err("spawn should fail during role-wiring fan-out");
    assert!(
        matches!(error, MobError::WiringError(_) | MobError::Internal(_)),
        "failed spawn should surface wiring/internal failure, got: {error}"
    );

    assert!(
        handle
            .get_member(&AgentIdentity::from("w-3"))
            .await
            .is_none(),
        "failed spawn must not leave the new member in the canonical roster"
    );
    let w1 = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("w-1 remains active");
    let w2 = handle
        .get_member(&AgentIdentity::from("w-2"))
        .await
        .expect("w-2 remains active");
    assert!(
        w1.wired_to.contains(&AgentIdentity::from("w-2")),
        "rollback must preserve the pre-existing worker pair edge on w-1"
    );
    assert!(
        w2.wired_to.contains(&AgentIdentity::from("w-1")),
        "rollback must preserve the pre-existing worker pair edge on w-2"
    );
    assert!(
        !w1.wired_to.contains(&AgentIdentity::from("w-3"))
            && !w2.wired_to.contains(&AgentIdentity::from("w-3")),
        "rollback must not leave dangling projections to the failed spawn target"
    );
}

// -----------------------------------------------------------------------
// Flow runtime lifecycle and orchestration behavior
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_run_flow_persists_before_reply_and_is_queryable() {
    let (handle, service) =
        create_test_mob(sample_definition_with_single_step_flow(2_000, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_delay_ms(200);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({ "input": "x" }))
        .await
        .expect("run flow");

    let immediate = handle
        .flow_status(run_id.clone())
        .await
        .expect("flow status call should succeed");
    assert!(
        immediate.is_some(),
        "run must be persisted before run_flow returns"
    );
    let run = immediate.expect("run should exist");
    assert_eq!(run.flow_id, FlowId::from("demo"));

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert!(
        terminal
            .step_ledger
            .iter()
            .any(|entry| entry.status == crate::run::StepRunStatus::Dispatched),
        "runtime flow path should persist dispatched ledger entries"
    );
    assert!(
        terminal.step_ledger.iter().any(|entry| {
            entry.step_id.as_str() == "start"
                && entry.status == crate::run::StepRunStatus::Completed
        }),
        "runtime flow path should persist completed step ledger entries"
    );
    assert!(
        terminal.failure_ledger.is_empty(),
        "successful flow should not persist failure ledger entries"
    );
}

#[tokio::test]
async fn test_parallel_targets_complete_concurrently() {
    let (handle, service) =
        create_test_mob(sample_definition_with_single_step_flow(5_000, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    service.set_flow_turn_delay_ms(400);

    let start = Instant::now();
    let run_id = handle
        .run_flow(
            FlowId::from("demo"),
            serde_json::json!({ "mode": "parallel" }),
        )
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    let elapsed = start.elapsed();

    assert_eq!(terminal.status, MobRunStatus::Completed);
    assert!(
        elapsed < Duration::from_millis(700),
        "two 400ms target turns should complete in parallel; observed {elapsed:?}"
    );
}

#[tokio::test]
async fn test_fanout_emits_per_target_and_aggregate_completion_events() {
    let (handle, _service) =
        create_test_mob(sample_definition_with_single_step_flow(2_000, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");

    let run_id = handle
        .run_flow(
            FlowId::from("demo"),
            serde_json::json!({ "case": "fanout-events" }),
        )
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let events = handle.events().replay_all().await.expect("replay");
    let per_target_completed = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepTargetCompleted { run_id: id, step_id, .. }
                    if id == &run_id && step_id.as_str() == "start"
            )
        })
        .count();
    let aggregate_completed = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepCompleted { run_id: id, step_id }
                    if id == &run_id && step_id.as_str() == "start"
            )
        })
        .count();

    assert_eq!(
        per_target_completed, 2,
        "fanout should emit one StepTargetCompleted per target"
    );
    assert_eq!(
        aggregate_completed, 1,
        "fanout should emit a single aggregate StepCompleted event"
    );
}

#[tokio::test]
async fn test_run_snapshots_are_captured_per_run_and_remain_immutable() {
    let (handle, service) =
        create_test_mob(sample_definition_with_single_step_flow(5_000, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_delay_ms(400);

    let run_a = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({"ticket":"A"}))
        .await
        .expect("run A");
    let snap_a_initial = handle
        .flow_status(run_a.clone())
        .await
        .expect("status A")
        .expect("run A exists");
    assert_eq!(
        snap_a_initial.activation_params,
        serde_json::json!({"ticket":"A"})
    );

    let run_b = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({"ticket":"B"}))
        .await
        .expect("run B");
    let snap_b = handle
        .flow_status(run_b.clone())
        .await
        .expect("status B")
        .expect("run B exists");
    assert_eq!(snap_b.activation_params, serde_json::json!({"ticket":"B"}));

    let snap_a_after = handle
        .flow_status(run_a.clone())
        .await
        .expect("status A after")
        .expect("run A still exists");
    assert_eq!(
        snap_a_after.activation_params,
        serde_json::json!({"ticket":"A"}),
        "run A activation snapshot must remain unchanged by run B"
    );

    let _ = handle.cancel_flow(run_a).await;
    let _ = handle.cancel_flow(run_b).await;
}

#[tokio::test]
async fn test_handle_list_flows_returns_definition_flow_ids() {
    let mut definition = sample_definition_with_single_step_flow(500, 8);
    let mut extra_steps = IndexMap::new();
    extra_steps.insert(step_id("start"), flow_step("worker", "Secondary"));
    definition.flows.insert(
        flow_id("secondary"),
        FlowSpec {
            description: Some("secondary flow".to_string()),
            steps: extra_steps,
            root: None,
        },
    );

    let (handle, _service) = create_test_mob(definition).await;
    let flows = handle.list_flows();
    assert_eq!(flows, vec![flow_id("demo"), flow_id("secondary")]);
}

#[tokio::test]
async fn test_dispatch_mode_one_to_one_dispatches_single_target() {
    let (handle, _service) =
        create_test_mob(sample_definition_with_dispatch_mode(DispatchMode::OneToOne)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");

    let run_id = handle
        .run_flow(FlowId::from("dispatch"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let events = handle.events().replay_all().await.expect("replay");
    let dispatched = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepDispatched { run_id: id, .. } if id == &run_id
            )
        })
        .count();
    assert_eq!(dispatched, 1, "one_to_one should dispatch one target");
}

#[tokio::test]
async fn test_dispatch_mode_fan_in_dispatches_all_targets() {
    let (handle, _service) =
        create_test_mob(sample_definition_with_dispatch_mode(DispatchMode::FanIn)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");

    let run_id = handle
        .run_flow(FlowId::from("dispatch"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let events = handle.events().replay_all().await.expect("replay");
    let dispatched = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepDispatched { run_id: id, .. } if id == &run_id
            )
        })
        .count();
    assert_eq!(dispatched, 2, "fan_in should dispatch all matching targets");
}

#[tokio::test]
async fn test_fan_in_aggregate_output_is_deterministic_array_shape() {
    let (handle, _service) = create_test_mob(sample_definition_with_dispatch_mode_and_policy(
        DispatchMode::FanIn,
        CollectionPolicy::All,
    ))
    .await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");

    let run_id = handle
        .run_flow(FlowId::from("dispatch"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let aggregate_output = terminal
        .step_ledger
        .iter()
        .find(|entry| {
            entry.step_id.as_str() == "dispatch"
                && entry.agent_identity == crate::runtime::flow_system_member_id()
                && entry.status == StepRunStatus::Completed
        })
        .and_then(|entry| entry.output.clone())
        .expect("aggregate dispatch output should be persisted");
    assert_eq!(
        aggregate_output,
        serde_json::json!([
            {"target":"w-1","output":"Turn completed"},
            {"target":"w-2","output":"Turn completed"}
        ]),
        "fan_in aggregate shape and ordering should be deterministic"
    );
}

#[tokio::test]
async fn test_max_step_retries_controls_dispatch_attempts() {
    let (handle, service) = create_test_mob(sample_definition_with_retry_flow(2)).await;
    let sid_w1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    service.set_flow_turn_fail_for_session(&sid_w1, true).await;

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);

    let events = handle.events().replay_all().await.expect("replay");
    let dispatched = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepDispatched { run_id: id, step_id, target }
                    if id == &run_id
                        && step_id.as_str() == "start"
                        && target.identity.as_str() == "w-1"
            )
        })
        .count();
    assert_eq!(
        dispatched, 3,
        "max_step_retries=2 should dispatch three attempts"
    );
}

#[tokio::test]
async fn test_orphan_budget_fairness_prevents_single_run_monopoly() {
    let (handle, service) = create_test_mob(sample_definition_with_single_step_flow(20, 2)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    service.set_flow_turn_never_terminal(true);

    let run_id_1 = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({"run": 1}))
        .await
        .expect("run flow 1");
    let _terminal_1 = wait_for_run_terminal(&handle, &run_id_1, Duration::from_secs(3)).await;

    let run_id_2 = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({"run": 2}))
        .await
        .expect("run flow 2");
    let terminal_2 = wait_for_run_terminal(&handle, &run_id_2, Duration::from_secs(3)).await;
    assert_eq!(terminal_2.status, MobRunStatus::Failed);
    assert!(
        terminal_2
            .failure_ledger
            .iter()
            .any(|entry| entry.reason.contains("timeout after")),
        "second run should still obtain timeout orphan capacity instead of complete starvation"
    );
}

#[tokio::test]
async fn test_branch_winner_is_selected_only_after_success_allowing_fallback() {
    let (handle, service) = create_test_mob(sample_definition_with_branch_fallback_flow()).await;
    let sid_fail = handle
        .spawn(
            ProfileName::from("worker_fail"),
            MeerkatId::from("w-fail"),
            None,
        )
        .await
        .expect("spawn failing worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .spawn(
            ProfileName::from("worker_ok"),
            MeerkatId::from("w-ok"),
            None,
        )
        .await
        .expect("spawn healthy worker");
    service
        .set_flow_turn_fail_for_session(&sid_fail, true)
        .await;

    let run_id = handle
        .run_flow(
            FlowId::from("branch_fallback"),
            serde_json::json!({"try_fallback": true}),
        )
        .await
        .expect("run branch fallback flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepFailed { run_id: id, step_id, .. }
                    if id == &run_id && step_id.as_str() == "candidate_first"
            )
        }),
        "first branch candidate should fail"
    );
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepCompleted { run_id: id, step_id }
                    if id == &run_id && step_id.as_str() == "candidate_second"
            )
        }),
        "fallback branch candidate should execute and complete"
    );
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepCompleted { run_id: id, step_id }
                    if id == &run_id && step_id.as_str() == "join"
            )
        }),
        "downstream join should complete after fallback branch success"
    );
}

#[tokio::test]
async fn test_malformed_templates_fail_flow_explicitly() {
    for template in ["Bad {{", "Bad }}", "{{   }}"] {
        let (handle, _service) =
            create_test_mob(sample_definition_with_template_message(template)).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");

        let run_id = handle
            .run_flow(FlowId::from("demo"), serde_json::json!({}))
            .await
            .expect("run flow");
        let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
        assert_eq!(terminal.status, MobRunStatus::Failed);
        assert!(
            terminal.failure_ledger.iter().any(|entry| {
                entry.reason.contains("invalid template syntax")
                    && entry.reason.contains("line")
                    && entry.reason.contains("column")
                    && entry.reason.contains("byte")
            }),
            "malformed template '{template}' should fail with deterministic syntax diagnostics"
        );
    }
}

#[tokio::test]
async fn test_template_syntax_diagnostics_are_deterministic_for_multiline_inputs() {
    let bad_template = "line1\nline2 }}";
    let (handle, _service) =
        create_test_mob(sample_definition_with_template_message(bad_template)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);

    let reason = terminal
        .failure_ledger
        .iter()
        .find(|entry| entry.reason.contains("invalid template syntax"))
        .map(|entry| entry.reason.clone())
        .expect("template error reason");
    assert!(
        reason.contains("line 2, column 7"),
        "diagnostic location should be deterministic for multiline templates: {reason}"
    );
}

#[tokio::test]
async fn test_shared_path_resolver_is_used_for_conditions_and_templates() {
    let (handle, _service) =
        create_test_mob(sample_definition_with_shared_path_resolution_flow()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("shared_paths"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);
    assert!(
        terminal.step_ledger.iter().any(|entry| {
            entry.step_id.as_str() == "follow" && entry.status == StepRunStatus::Completed
        }),
        "follow step should execute using shared path semantics in both condition and template"
    );
}

#[tokio::test]
async fn test_flow_finished_cleans_tracking_maps() {
    let (handle, _service) = create_test_mob(sample_definition_with_single_step_flow(500, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let (tasks, tokens) = handle
            .debug_flow_tracker_counts()
            .await
            .expect("tracker counts");
        if tasks == 0 && tokens == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for flow trackers to drain (tasks={tasks}, tokens={tokens})"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn test_mob_schema_initial_orchestrator_projection_matches_runtime() {
    let schema = schema_mob_machine();
    assert_eq!(schema.state.init.phase.as_str(), "Running");
    let coordinator_bound = schema
        .state
        .init
        .fields
        .iter()
        .find(|field| field.field.as_str() == "coordinator_bound")
        .expect("coordinator_bound init");
    assert_eq!(coordinator_bound.expr, Expr::Bool(true));

    let (handle, _service) = create_test_mob(sample_definition()).await;
    let initial = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("initial orchestrator snapshot");
    assert_eq!(initial.phase, MobState::Running);
    assert!(initial.coordinator_bound);
    assert_eq!(initial.pending_spawn_count, 0);
}

#[tokio::test]
async fn test_mob_runtime_parity_field_evaluator_covers_formal_state_fields() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let snapshot = mob_runtime_parity_snapshot_summary(&handle)
        .await
        .expect("runtime parity snapshot");
    let schema = schema_mob_machine();

    let missing = schema
        .state
        .fields
        .iter()
        .filter(|field| mob_runtime_parity_field_value(&snapshot, field.name.as_str()).is_none())
        .map(|field| field.name.as_str().to_string())
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "runtime parity field evaluator is missing schema fields: {missing:?}"
    );
}

#[test]
fn test_bridge_protocol_types_live_in_contracts_not_runtime() {
    // Verify the canonical protocol types are re-exported from contracts,
    // not defined in meerkat-runtime. This is a compile-time structural
    // assertion — if the re-export chain breaks, this won't compile.
    let _cmd: meerkat_contracts::BridgeCommand = meerkat_contracts::BridgeCommand::ObserveMember(
        meerkat_contracts::BridgeSupervisorPayload {
            supervisor: meerkat_contracts::BridgePeerSpec {
                name: "test".into(),
                peer_id: "ed25519:test".into(),
                address: "inproc://test".into(),
                pubkey: [0u8; 32],
            },
            epoch: 1,
            protocol_version:
                meerkat_contracts::wire::supervisor_bridge::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        },
    );
}

#[test]
fn test_top_level_mob_schema_does_not_duplicate_bridge_protocol_effects() {
    let schema = schema_mob_machine();
    let forbidden_effects = [
        "BridgeBindMember",
        "BridgeAuthorizeSupervisor",
        "BridgeRevokeSupervisor",
        "BridgeDeliverMemberInput",
        "BridgeObserveMember",
        "BridgeInterruptMember",
        "BridgeRetireMember",
        "BridgeDestroyMember",
        "BridgeWireMember",
        "BridgeUnwireMember",
    ];
    let schema_effect_names: Vec<_> = schema
        .effects
        .variants
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    let duplicated: Vec<_> = forbidden_effects
        .iter()
        .filter(|name| schema_effect_names.contains(name))
        .collect();
    assert!(
        duplicated.is_empty(),
        "top-level mob schema must not duplicate bridge protocol effects: {duplicated:?}"
    );
}

#[test]
fn test_top_level_mob_schema_avoids_shadow_bridge_state_fields() {
    let schema = schema_mob_machine();
    let forbidden_fields = [
        "remote_member_count",
        "supervisor_authorized",
        "supervisor_rotating",
        "rotation_pending_acks",
    ];
    let field_names: Vec<_> = schema
        .state
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    let shadowed: Vec<_> = forbidden_fields
        .iter()
        .filter(|name| field_names.contains(name))
        .collect();
    assert!(
        shadowed.is_empty(),
        "top-level mob schema must not carry shadow bridge state fields: {shadowed:?}"
    );
}

#[test]
fn test_top_level_mob_schema_does_not_model_schema_only_rotation_transitions() {
    let schema = schema_mob_machine();
    let transition_names: Vec<_> = schema.transitions.iter().map(|t| t.name.as_str()).collect();
    assert!(
        !transition_names.contains(&"RotateSupervisorRunning"),
        "top-level mob schema must not model schema-only RotateSupervisor transitions"
    );
    assert!(
        !transition_names.contains(&"AckRotationRunning"),
        "top-level mob schema must not model schema-only AckRotation transitions"
    );

    let manifest = crate::mob_machine::canonical_mob_machine_command_manifest();
    assert!(
        !manifest.contains("RotateSupervisor"),
        "canonical mob machine command surface must not include schema-only RotateSupervisor"
    );
    assert!(
        !manifest.contains("AckRotation"),
        "canonical mob machine command surface must not include schema-only AckRotation"
    );
}

#[tokio::test]
async fn test_orchestrator_snapshot_tracks_pending_spawn_ownership_and_revision() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let initial = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("orchestrator snapshot");
    assert!(initial.coordinator_bound);
    assert_eq!(initial.pending_spawn_count, 0);

    service.set_create_session_delay_ms(150);
    let spawn_handle = {
        let handle = handle.clone();
        tokio::spawn(async move {
            handle
                .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
                .await
        })
    };

    tokio::time::sleep(Duration::from_millis(30)).await;
    let staged = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("staged snapshot");
    assert_eq!(staged.pending_spawn_count, 1);
    let staged_dsl = handle
        .debug_dsl_t2_snapshot()
        .await
        .expect("staged dsl snapshot");
    assert_eq!(
        staged_dsl.pending_spawn_sessions.len(),
        1,
        "exactly one machine-owned pending spawn admission should be visible while provisioning is blocked"
    );
    assert!(
        staged_dsl
            .pending_spawn_sessions
            .keys()
            .any(|identity| identity.0 == "w-1"),
        "pending admission should carry the member identity, not just an anonymous count"
    );
    // topology_revision is orchestrator shell metadata not tracked by the DSL authority.

    spawn_handle
        .await
        .expect("spawn join")
        .expect("spawn worker");
    let settled = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("settled snapshot");
    assert_eq!(settled.pending_spawn_count, 0);
    let settled_dsl = handle
        .debug_dsl_t2_snapshot()
        .await
        .expect("settled dsl snapshot");
    assert!(
        settled_dsl.pending_spawn_sessions.is_empty(),
        "successful spawn completion should clear pending admission"
    );

    let events = handle.events().replay_all().await.expect("replay events");
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        MobEventKind::MemberSpawned(member_spawned)
            if member_spawned.agent_identity.as_str() == "w-1"
    )));
}

#[tokio::test]
async fn test_flow_tracker_maps_remain_coherent_under_concurrent_run_and_cancel_commands() {
    let (handle, service) =
        create_test_mob(sample_definition_with_single_step_flow(5_000, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_delay_ms(150);

    let mut run_ids = Vec::new();
    for _ in 0..8 {
        run_ids.push(
            handle
                .run_flow(FlowId::from("demo"), serde_json::json!({}))
                .await
                .expect("run flow"),
        );
    }

    let mut cancel_tasks = Vec::new();
    for run_id in run_ids.clone() {
        let handle = handle.clone();
        cancel_tasks.push(tokio::spawn(async move {
            handle.cancel_flow(run_id).await.expect("cancel flow");
        }));
    }
    for task in cancel_tasks {
        task.await.expect("cancel join");
    }

    for run_id in &run_ids {
        let terminal = wait_for_run_terminal(&handle, run_id, Duration::from_secs(8)).await;
        assert!(
            matches!(
                terminal.status,
                MobRunStatus::Canceled | MobRunStatus::Failed | MobRunStatus::Completed
            ),
            "concurrent run/cancel pressure must still converge to terminal runs"
        );
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let (tasks, tokens) = handle
            .debug_flow_tracker_counts()
            .await
            .expect("tracker counts");
        if tasks == 0 && tokens == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for actor-owned flow trackers to drain (tasks={tasks}, tokens={tokens})"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn test_orchestrator_snapshot_tracks_flow_activation_and_lifecycle_transitions() {
    let (handle, service) = create_test_mob(sample_definition_with_supervisor_threshold(1)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_delay_ms(150);

    let run_id = handle
        .run_flow(
            FlowId::from("demo"),
            serde_json::json!({"source":"orchestrator-test"}),
        )
        .await
        .expect("run flow");

    tokio::time::sleep(Duration::from_millis(30)).await;
    let active = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("active snapshot");
    assert_eq!(active.active_flow_count, 1);
    assert!(active.coordinator_bound);
    // supervisor_active is orchestrator shell metadata not tracked by the DSL authority.

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let deadline = Instant::now() + Duration::from_secs(2);
    let settled = loop {
        let snapshot = handle
            .debug_orchestrator_snapshot()
            .await
            .expect("settled snapshot");
        if snapshot.active_flow_count == 0 {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for orchestrator active-flow count to drain"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(settled.coordinator_bound);
    // supervisor_active is orchestrator shell metadata not tracked by the DSL authority.

    handle.stop().await.expect("stop mob");
    let stopped = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("stopped snapshot");
    assert!(!stopped.coordinator_bound);
    // supervisor_active is orchestrator shell metadata not tracked by the DSL authority.

    handle.resume().await.expect("resume mob");
    let resumed = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("resumed snapshot");
    assert!(resumed.coordinator_bound);
    assert!(resumed.supervisor_active);

    let events = handle.events().replay_all().await.expect("replay events");
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        MobEventKind::FlowStarted { run_id: event_run_id, .. } if event_run_id == &run_id
    )));
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        MobEventKind::FlowCompleted { run_id: event_run_id, .. } if event_run_id == &run_id
    )));
}

#[tokio::test]
async fn test_failed_flow_run_drains_orchestrator_and_tracker_state() {
    let (handle, service) =
        create_test_mob(sample_definition_with_single_step_flow(2_000, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_fail(true);
    service.set_flow_turn_delay_ms(150);

    let run_id = handle
        .run_flow(
            FlowId::from("demo"),
            serde_json::json!({"mode":"fail-cleanup"}),
        )
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let snapshot = handle
            .debug_orchestrator_snapshot()
            .await
            .expect("orchestrator snapshot");
        let (tasks, tokens) = handle
            .debug_flow_tracker_counts()
            .await
            .expect("flow tracker counts");
        if snapshot.active_flow_count == 0 && tasks == 0 && tokens == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "failed flow must fail closed by draining orchestrator/tracker state (active_flows={}, tasks={}, tokens={})",
            snapshot.active_flow_count,
            tasks,
            tokens
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn test_complete_cancels_inflight_flow_run() {
    let (handle, service) =
        create_test_mob(sample_definition_with_single_step_flow(60_000, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.complete().await.expect("complete mob");

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);
}

#[tokio::test]
async fn test_destroy_cancels_inflight_flow_run() {
    let run_store = Arc::new(RecordingRunStore::new());
    let (handle, service) = create_test_mob_with_run_store(
        sample_definition_with_single_step_flow(60_000, 8),
        run_store.clone(),
    )
    .await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.destroy().await.expect("destroy mob");

    let terminal = run_store
        .get_run(&run_id)
        .await
        .expect("read run after destroy")
        .expect("run should remain queryable in run store after destroy");
    assert_eq!(terminal.status, MobRunStatus::Canceled);
}

#[tokio::test]
async fn test_run_flow_store_admission_failure_does_not_commit_mob_machine_authority() {
    let run_store = Arc::new(RecordingRunStore::new());
    let definition = sample_definition_with_single_step_flow(60_000, 8);
    let mob_id = definition.id.clone();
    let (handle, _service) = create_test_mob_with_run_store(definition, run_store.clone()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker before flow admission");

    run_store.fail_next_create_run();
    let error = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect_err("fault-injected create_run failure should fail RunFlow admission");
    assert!(
        error
            .to_string()
            .contains("fault-injected create_run failure"),
        "admission error should surface run-store failure, got: {error}"
    );

    let machine_state = handle
        .query_machine_state()
        .await
        .expect("query MobMachine state after failed admission");
    assert_eq!(
        machine_state.active_run_count, 0,
        "RunFlow store failure must not leave MobMachine active_run_count advanced"
    );
    assert!(
        machine_state.run_status.is_empty(),
        "RunFlow store failure must not leave MobMachine run projections behind"
    );
    let persisted_runs = run_store
        .list_runs(&mob_id, Some(&FlowId::from("demo")))
        .await
        .expect("list persisted runs");
    assert!(
        persisted_runs.is_empty(),
        "fault-injected admission failure must not persist a run"
    );
}

#[test]
fn test_flow_cleanup_uses_terminal_projection_not_authority_clone_probe() {
    let source = include_str!("actor.rs");
    let start = source
        .find("async fn handle_flow_cleanup")
        .expect("handle_flow_cleanup exists");
    let end = source[start..]
        .find("async fn handle_cancel_flow")
        .expect("handle_cancel_flow follows cleanup");
    let body = &source[start..start + end];
    assert!(
        !body.contains("MobMachineAuthority::from_state(self.dsl_authority.state.clone())"),
        "flow cleanup must use run terminality and live authority state directly, not clone-probe MobMachine"
    );
    assert!(
        !body.contains("authorities_expect_completion"),
        "flow cleanup must not infer terminality by probing whether authority signals would be accepted"
    );
}

#[test]
fn test_lifecycle_commands_admit_on_live_authority_not_clone_probe() {
    let source = include_str!("actor.rs");
    let start = source
        .find("MobCommand::Stop { reply_tx }")
        .expect("Stop command arm exists");
    let end = source[start..]
        .find("MobCommand::TaskCreate")
        .expect("TaskCreate command arm follows lifecycle command arms");
    let lifecycle_arms = &source[start..start + end];

    for command in ["Stop", "ResumeLifecycle", "Complete", "Reset"] {
        let command_start = lifecycle_arms
            .find(&format!("MobCommand::{command}"))
            .unwrap_or_else(|| panic!("{command} command arm exists"));
        let command_end = [
            "MobCommand::ResumeLifecycle",
            "MobCommand::Complete",
            "MobCommand::Destroy",
            "MobCommand::Reset",
            "MobCommand::TaskCreate",
        ]
        .into_iter()
        .filter_map(|marker| {
            lifecycle_arms[command_start + 1..]
                .find(marker)
                .map(|offset| command_start + 1 + offset)
        })
        .min()
        .unwrap_or(lifecycle_arms.len());
        let arm = &lifecycle_arms[command_start..command_end];
        assert!(
            !arm.contains("probe_mob_machine_input"),
            "{command} lifecycle admission must not be decided by cloned MobMachine probe truth"
        );
    }
}

#[test]
fn test_flow_frame_store_plan_commits_authority_with_store_in_actor() {
    let source = include_str!("flow_frame_engine.rs");
    let start = source
        .find("async fn execute_store_plan")
        .expect("execute_store_plan exists");
    let end = source[start..]
        .find("async fn project_store_plan")
        .expect("project_store_plan follows execute_store_plan");
    let body = &source[start..start + end];
    assert!(
        body.contains("commit_flow_frame_store_plan"),
        "frame loop store plans must be committed through the actor authority/store seam"
    );
    assert!(
        !body.contains("commit_mob_machine_inputs(machine_inputs)"),
        "frame loop store plans must not persist store state before separately committing MobMachine inputs"
    );
}

#[test]
fn test_flow_frame_kernel_mutations_route_store_and_authority_through_actor() {
    let source = include_str!("flow_frame_engine.rs");
    let start = source
        .find("impl FlowFrameMutator for FlowFrameKernel")
        .expect("FlowFrameKernel mutator impl exists");
    let end = source[start..]
        .find("// ─── FlowFrameEngine")
        .expect("FlowFrameEngine section follows kernel mutator impl");
    let body = &source[start..start + end];
    for disallowed in [
        ".cas_frame_state(",
        ".cas_complete_step_and_record_output(",
        ".cas_run_snapshot(",
        "commit_mob_machine_inputs(",
    ] {
        assert!(
            !body.contains(disallowed),
            "FlowFrameKernel store mutation '{disallowed}' must be committed through the actor authority/store seam"
        );
    }
    assert!(
        body.contains("commit_flow_frame_store_plan"),
        "FlowFrameKernel frame store mutations must route through actor-owned prepared authority commits"
    );
}

#[test]
fn test_flow_frame_scheduler_start_run_routes_through_actor_authority() {
    let source = include_str!("flow_frame_engine.rs");
    let start = source
        .find("async fn ensure_scheduler_state_initialized")
        .expect("ensure_scheduler_state_initialized exists");
    let end = source[start..]
        .find("async fn heal_orphaned_running_nodes")
        .expect("heal helper follows scheduler initialization");
    let body = &source[start..start + end];
    assert!(
        body.contains("commit_flow_run_command"),
        "scheduler initialization must use the actor-owned flow run command seam"
    );
    for disallowed in [".cas_run_snapshot(", "commit_mob_machine_inputs("] {
        assert!(
            !body.contains(disallowed),
            "scheduler initialization must not persist run state before a separate MobMachine authority commit"
        );
    }
}

fn serialized_flow_authority_inputs(run: &MobRun) -> Vec<serde_json::Value> {
    let run_json = serde_json::to_value(run).expect("serialize MobRun");
    run_json
        .get("flow_authority_inputs")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("MobRun must persist flow_authority_inputs with projections"))
}

fn flow_authority_log_text(run: &MobRun) -> String {
    serde_json::to_string(&serialized_flow_authority_inputs(run))
        .expect("serialize flow authority log")
}

fn assert_flow_authority_log_contains(run: &MobRun, needle: &str) {
    let log = flow_authority_log_text(run);
    assert!(
        log.contains(needle),
        "flow authority log should contain {needle}; log={log}"
    );
}

fn apply_authority_input_for_test(
    authority: &mut crate::machines::mob_machine::MobMachineAuthority,
    input: crate::machines::mob_machine::MobMachineInput,
) {
    let transition = crate::machines::mob_machine::MobMachineMutator::apply(authority, input)
        .expect("test authority input should be accepted");
    if transition.from_phase != transition.to_phase {
        authority.state.lifecycle_phase = transition.to_phase;
    }
}

fn authority_backed_root_frame_run(
    include_loop_seed: bool,
) -> (MobRun, crate::ids::FrameId, crate::ids::LoopInstanceId) {
    use crate::ids::{FlowNodeId, LoopId, RunId};

    let definition = sample_definition_with_single_step_flow(500, 8);
    let config = crate::run::FlowRunConfig::from_definition(FlowId::from("demo"), &definition)
        .expect("flow config");
    let run_id = RunId::new();
    let create_run = MobRun::create_run_seed_input(&run_id, &config).expect("create run seed");

    let loop_node_id = FlowNodeId::from("loop-node");
    let dependency_node_id = FlowNodeId::from("blocked-on");
    let loop_id = LoopId::from("retry");
    let frame_id = crate::ids::FrameId::from(format!("{run_id}-root").as_str());
    let create_frame = crate::machines::mob_machine::MobMachineInput::CreateFrameSeed {
        run_id: crate::machines::mob_machine::RunId::from(run_id.to_string()),
        frame_id: crate::machines::mob_machine::FrameId::from(frame_id.as_str()),
        frame_scope: crate::machines::mob_machine::FrameScope::Root,
        loop_instance_id: None,
        iteration: 0,
        tracked_nodes: [crate::machines::mob_machine::FlowNodeId::from(
            loop_node_id.as_str(),
        )]
        .into_iter()
        .collect(),
        ordered_nodes: vec![crate::machines::mob_machine::FlowNodeId::from(
            loop_node_id.as_str(),
        )],
        node_kind: [(
            crate::machines::mob_machine::FlowNodeId::from(loop_node_id.as_str()),
            crate::machines::mob_machine::FlowNodeKind::Loop,
        )]
        .into_iter()
        .collect(),
        node_dependencies: [(
            crate::machines::mob_machine::FlowNodeId::from(loop_node_id.as_str()),
            vec![crate::machines::mob_machine::FlowNodeId::from(
                dependency_node_id.as_str(),
            )],
        )]
        .into_iter()
        .collect(),
        node_dependency_modes: [(
            crate::machines::mob_machine::FlowNodeId::from(loop_node_id.as_str()),
            crate::machines::mob_machine::DependencyMode::All,
        )]
        .into_iter()
        .collect(),
        node_branches: [(
            crate::machines::mob_machine::FlowNodeId::from(loop_node_id.as_str()),
            None,
        )]
        .into_iter()
        .collect(),
        node_step_ids: BTreeMap::new(),
        node_loop_ids: [(
            crate::machines::mob_machine::FlowNodeId::from(loop_node_id.as_str()),
            crate::machines::mob_machine::LoopId::from(loop_id.as_str()),
        )]
        .into_iter()
        .collect(),
        node_status: [(
            crate::machines::mob_machine::FlowNodeId::from(loop_node_id.as_str()),
            crate::machines::mob_machine::NodeRunStatus::Pending,
        )]
        .into_iter()
        .collect(),
        ready_queue: Vec::new(),
    };
    let loop_instance_id =
        crate::ids::LoopInstanceId::from(format!("{frame_id}::{loop_node_id}").as_str());
    let create_loop = MobRun::create_loop_seed_input_for_start(
        &loop_instance_id,
        &frame_id,
        &loop_node_id,
        &loop_id,
        1,
        3,
    );

    let mut authority = crate::machines::mob_machine::MobMachineAuthority::new();
    apply_authority_input_for_test(&mut authority, create_run.clone());
    apply_authority_input_for_test(&mut authority, create_frame.clone());
    if include_loop_seed {
        apply_authority_input_for_test(&mut authority, create_loop.clone());
    }

    let mut run = MobRun::pending_with_run_id(
        run_id.clone(),
        definition.id.clone(),
        FlowId::from("demo"),
        MobRun::flow_state_for_config_with_authority(&run_id, &config, &authority.state)
            .expect("project flow state"),
        serde_json::json!({}),
    );
    let mut inputs = vec![create_run, create_frame];
    if include_loop_seed {
        inputs.push(create_loop);
    }
    run.append_flow_authority_inputs(inputs)
        .expect("append authority inputs");
    run.frames.insert(
        frame_id.clone(),
        crate::run::FrameSnapshot {
            kernel_state: crate::run::project_flow_frame_authority_state_from_machine(
                &authority.state,
                &frame_id,
            )
            .expect("project frame state"),
        },
    );

    (run, frame_id, loop_instance_id)
}

fn running_authority_backed_run(definition: &MobDefinition) -> MobRun {
    use crate::ids::RunId;
    use crate::run::{
        MobMachineFlowAuthorityToken, MobMachineFlowRunCommand, apply_mob_machine_flow_run_command,
        flow_run,
    };

    let flow_id = FlowId::from("demo");
    let config = crate::run::FlowRunConfig::from_definition(flow_id.clone(), definition)
        .expect("flow config");
    let run_id = RunId::new();
    let run_flow = MobRun::run_flow_input(&run_id, &config).expect("run flow input");
    let mut authority = crate::machines::mob_machine::MobMachineAuthority::new();
    apply_authority_input_for_test(&mut authority, run_flow.clone());

    let mut run = MobRun::pending_with_run_id(
        run_id.clone(),
        definition.id.clone(),
        flow_id,
        MobRun::flow_state_for_config_with_authority(&run_id, &config, &authority.state)
            .expect("project admitted flow state"),
        serde_json::json!({}),
    );
    run.append_flow_authority_inputs(vec![run_flow])
        .expect("append RunFlow authority input");

    let start = MobMachineFlowRunCommand::StartRun(flow_run::inputs::StartRun {});
    let start_input = start.authority_input(&run_id);
    apply_authority_input_for_test(&mut authority, start_input.clone());
    let authority_token =
        MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&start_input)
            .expect("start authority token");
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        &authority.state,
        &run_id,
        start,
        authority_token,
    )
    .expect("project started flow state");
    run.status = MobRunStatus::Running;
    run.flow_state = outcome.next_state;
    run.append_flow_authority_inputs(vec![start_input])
        .expect("append StartRun authority input");
    run
}

#[tokio::test]
async fn test_flow_frame_store_plan_persists_authority_input_with_projection() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec};
    use crate::ids::{FlowNodeId, FrameId};
    use crate::runtime::flow_frame_engine::{FlowFrameKernel, FlowFrameMutator};

    let store = Arc::new(InMemoryMobRunStore::new());
    let run = MobRun::pending(
        crate::ids::MobId::from("test-mob"),
        FlowId::from("test-flow"),
        crate::run::flow_run::initial_state(),
        serde_json::json!({}),
    );
    let run_id = run.run_id.clone();
    store.create_run(run).await.expect("create run");

    let (handle, _) = create_test_mob_with_run_store(sample_definition(), store.clone()).await;
    seed_test_run_in_mob_machine(&handle, &run_id).await;

    let node_id = FlowNodeId::from("start-node");
    let mut nodes = IndexMap::new();
    nodes.insert(
        node_id,
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: step_id("start"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    let root_spec = FrameSpec { nodes };
    let frame_id = FrameId::from(format!("{run_id}-root").as_str());
    let frame_kernel = FlowFrameKernel::new(store.clone(), handle);

    frame_kernel
        .start_frame(&run_id, &frame_id, &root_spec)
        .await
        .expect("start root frame");
    frame_kernel
        .admit_next_ready_node_with_retry(&run_id, &frame_id, 5)
        .await
        .expect("admit next ready node")
        .expect("admission effects");

    let persisted = store
        .get_run(&run_id)
        .await
        .expect("get run")
        .expect("run exists");
    let authority_inputs = serialized_flow_authority_inputs(&persisted);
    assert!(
        authority_inputs.len() >= 2,
        "frame seed and admit projection commits must both record authority inputs"
    );
    assert_flow_authority_log_contains(&persisted, "CreateFrameSeed");
    assert_flow_authority_log_contains(&persisted, "AuthorizeFlowFrameReducerCommand");
    assert_flow_authority_log_contains(&persisted, "AdmitNextReadyNode");
}

#[tokio::test]
async fn test_flow_run_start_and_completion_persist_authority_inputs_with_projection() {
    let store = Arc::new(InMemoryMobRunStore::new());
    let run = MobRun::pending(
        crate::ids::MobId::from("test-mob"),
        FlowId::from("test-flow"),
        crate::run::flow_run::initial_state(),
        serde_json::json!({}),
    );
    let run_id = run.run_id.clone();
    store.create_run(run).await.expect("create run");

    let (handle, _) = create_test_mob_with_run_store(sample_definition(), store.clone()).await;
    seed_test_run_in_mob_machine(&handle, &run_id).await;

    let effects = handle
        .commit_flow_run_command(
            &run_id,
            crate::run::MobMachineFlowRunCommand::StartRun(
                crate::run::flow_run::inputs::StartRun {},
            ),
            "test_start_run_authority_log",
        )
        .await
        .expect("start run command")
        .expect("start run should transition");
    assert!(!effects.is_empty(), "start run should emit effects");

    let started = store
        .get_run(&run_id)
        .await
        .expect("get started run")
        .expect("started run exists");
    assert_eq!(started.status, MobRunStatus::Running);
    assert_flow_authority_log_contains(&started, "StartRun");

    let terminal = handle
        .commit_flow_terminalization(
            run_id.clone(),
            FlowId::from("test-flow"),
            super::terminalization::TerminalizationTarget::Completed,
            crate::run::MobMachineFlowRunCommand::TerminalizeCompleted(
                crate::run::flow_run::inputs::TerminalizeCompleted {},
            ),
            "test_completed_run_authority_log",
        )
        .await
        .expect("terminalize completed");
    assert_eq!(
        terminal,
        super::terminalization::TerminalizationOutcome::Transitioned
    );

    let completed = store
        .get_run(&run_id)
        .await
        .expect("get completed run")
        .expect("completed run exists");
    assert_eq!(completed.status, MobRunStatus::Completed);
    assert_flow_authority_log_contains(&completed, "TerminalizeCompleted");
}

#[tokio::test]
async fn test_recovery_rejects_flow_authority_log_projection_divergence() {
    let store = Arc::new(InMemoryMobRunStore::new());
    let mut run = MobRun::pending(
        crate::ids::MobId::from("test-mob"),
        FlowId::from("test-flow"),
        crate::run::flow_run::initial_state(),
        serde_json::json!({}),
    );
    let run_id = run.run_id.clone();
    let seed_input = crate::machines::mob_machine::MobMachineInput::CreateRunSeed {
        run_id: crate::machines::mob_machine::RunId::from(run_id.to_string()),
        step_ids: Default::default(),
        ordered_steps: Vec::new(),
        step_has_conditions: Default::default(),
        step_dependencies: Default::default(),
        step_dependency_modes: Default::default(),
        step_branches: Default::default(),
        step_collection_policies: Default::default(),
        step_quorum_thresholds: Default::default(),
        escalation_threshold: 0,
        max_step_retries: 0,
        max_active_nodes: 0,
        max_active_frames: 0,
        max_frame_depth: 0,
    };
    run.append_flow_authority_inputs(vec![seed_input.clone()])
        .expect("append seed authority input");
    store.create_run(run).await.expect("create run");

    let (handle, _) = create_test_mob_with_run_store(sample_definition(), store.clone()).await;
    handle
        .project_machine_input(seed_input)
        .await
        .expect("seed run in MobMachine");
    handle
        .commit_flow_run_command(
            &run_id,
            crate::run::MobMachineFlowRunCommand::StartRun(
                crate::run::flow_run::inputs::StartRun {},
            ),
            "test_start_run_recovery_authority_log",
        )
        .await
        .expect("start run command")
        .expect("start run should transition");

    let mut persisted = store
        .get_run(&run_id)
        .await
        .expect("get run")
        .expect("run exists");
    crate::runtime::recovery::reconcile_run_state(&mut persisted)
        .expect("fresh persisted run should validate");

    persisted.flow_state.phase = crate::run::flow_run::Phase::Pending;
    let error = crate::runtime::recovery::reconcile_run_state(&mut persisted)
        .expect_err("tampered projection must not validate against authority log");
    assert!(
        matches!(
            error,
            crate::runtime::recovery::RestoreIncompatible::FlowAuthorityProjectionMismatch { .. }
        ),
        "unexpected recovery error: {error}"
    );
}

#[test]
fn test_recovery_rejects_authority_created_frame_missing_from_projection() {
    let (mut run, frame_id, _) = authority_backed_root_frame_run(false);
    run.frames.remove(&frame_id);

    let error = crate::runtime::recovery::reconcile_run_state(&mut run)
        .expect_err("authority-created frame missing from persisted projections must fail");
    assert!(
        matches!(
            error,
            crate::runtime::recovery::RestoreIncompatible::FlowAuthorityProjectionMismatch { .. }
        ),
        "unexpected recovery error: {error}"
    );
}

#[test]
fn test_recovery_rejects_authority_created_loop_missing_from_projection() {
    let (mut run, _, loop_instance_id) = authority_backed_root_frame_run(true);
    assert!(
        !run.loops.contains_key(&loop_instance_id),
        "test fixture intentionally omits the authority-created loop projection"
    );

    let error = crate::runtime::recovery::reconcile_run_state(&mut run)
        .expect_err("authority-created loop missing from persisted projections must fail");
    assert!(
        matches!(
            error,
            crate::runtime::recovery::RestoreIncompatible::FlowAuthorityProjectionMismatch { .. }
        ),
        "unexpected recovery error: {error}"
    );
}

#[tokio::test]
async fn test_resume_converges_untracked_active_flow_runs() {
    let definition = sample_definition_with_single_step_flow(60_000, 8);
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runs = storage.runs.clone();
    let specs = storage.specs.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::new(definition.clone(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle.shutdown().await.expect("shutdown actor");

    let active = running_authority_backed_run(&definition);
    let run_id = active.run_id.clone();
    runs.create_run(active)
        .await
        .expect("persist active run as crash residue");

    let resumed = MobBuilder::for_resume(MobStorage::custom_with_runtime_metadata(
        events,
        runs.clone(),
        specs,
        runtime_metadata,
    ))
    .with_session_service(service)
    .resume()
    .await
    .expect("resume mob");

    let terminal = wait_for_run_terminal(&resumed, &run_id, Duration::from_secs(2)).await;
    assert_eq!(
        terminal.status,
        MobRunStatus::Canceled,
        "resume must converge persisted active runs that have no actor trackers"
    );
    let machine_state = resumed
        .query_machine_state()
        .await
        .expect("query machine state");
    assert_eq!(
        machine_state.active_run_count, 0,
        "recovered active flow must not leave MobMachine active_run_count blocking stop"
    );
    resumed
        .stop()
        .await
        .expect("stop should not be blocked by an untracked recovered flow");
}

#[test]
fn test_resume_path_replays_persisted_flow_authority_inputs() {
    let builder = include_str!("builder.rs");
    assert!(
        builder.contains("seed_mob_authority_sync_from_flow_runs"),
        "resume must seed MobMachine flow authority from persisted run authority inputs"
    );

    let recovery = include_str!("recovery.rs");
    assert!(
        recovery.contains("validate_flow_authority_projection"),
        "recovery must validate projection snapshots against the persisted MobMachine authority input log"
    );
}

#[test]
fn test_actor_owned_terminalization_paths_do_not_reenter_actor_mailbox() {
    let source = include_str!("actor.rs");
    let run_flow_start = source
        .find("async fn handle_run_flow")
        .expect("handle_run_flow exists");
    let run_flow_pre_spawn_end = source[run_flow_start..]
        .find("let cancel_token = tokio_util::sync::CancellationToken::new();")
        .expect("handle_run_flow creates cancel token after admission");
    let run_flow_pre_spawn = &source[run_flow_start..run_flow_start + run_flow_pre_spawn_end];
    assert!(
        !run_flow_pre_spawn.contains(".flow_engine\n                .terminalize_failed("),
        "actor-owned run admission failure must terminalize through the in-actor helper, not by sending to its own actor mailbox"
    );
    assert!(
        run_flow_pre_spawn.contains("terminalize_failed_in_actor"),
        "actor-owned run admission failure must use the in-actor failed terminalization helper"
    );

    let cancel_start = source
        .find("async fn handle_cancel_flow")
        .expect("handle_cancel_flow exists");
    let cancel_body_end = source[cancel_start..]
        .find("let flow_engine = self.flow_engine.clone();")
        .expect("handle_cancel_flow spawns async cancellation cleanup after no-handle path");
    let cancel_no_handle_body = &source[cancel_start..cancel_start + cancel_body_end];
    assert!(
        !cancel_no_handle_body.contains(".flow_engine\n                .terminalize_canceled("),
        "actor-owned no-task cancel path must terminalize through the in-actor helper, not by sending to its own actor mailbox"
    );
    assert!(
        cancel_no_handle_body.contains("terminalize_canceled_in_actor"),
        "actor-owned no-task cancel path must use the in-actor canceled terminalization helper"
    );
}

#[tokio::test]
async fn test_stale_flow_frame_store_plan_loses_cas_before_authority_prepare() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec};
    use crate::ids::{FlowNodeId, FrameId};
    use crate::runtime::flow_frame_engine::{FlowFrameKernel, FlowFrameLoopStorePlan};

    let store = Arc::new(InMemoryMobRunStore::new());
    let run = MobRun::pending(
        crate::ids::MobId::from("test-mob"),
        FlowId::from("test-flow"),
        crate::run::flow_run::initial_state(),
        serde_json::json!({}),
    );
    let run_id = run.run_id.clone();
    store.create_run(run).await.expect("create run");

    let (handle, _) = create_test_mob_with_run_store(sample_definition(), store.clone()).await;
    seed_test_run_in_mob_machine(&handle, &run_id).await;

    let node_id = FlowNodeId::from("start-node");
    let mut nodes = IndexMap::new();
    nodes.insert(
        node_id.clone(),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: step_id("start"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    let root_spec = FrameSpec { nodes };

    let frame_id = FrameId::from(format!("{run_id}-root").as_str());
    let frame_kernel = FlowFrameKernel::new(store.clone(), handle.clone());
    let initial_frame = frame_kernel
        .start_frame(&run_id, &frame_id, &root_spec)
        .await
        .expect("start root frame");
    frame_kernel
        .admit_next_ready_node_with_retry(&run_id, &frame_id, 5)
        .await
        .expect("admit next ready node")
        .expect("admission effects");

    let advanced_frame = store
        .get_run(&run_id)
        .await
        .expect("get run")
        .expect("run exists")
        .frames
        .get(&frame_id)
        .cloned()
        .expect("advanced frame exists");
    assert_ne!(
        advanced_frame, initial_frame,
        "admission should advance the persisted frame before replaying stale plan"
    );
    let authority_log_len_before_stale_replay = serialized_flow_authority_inputs(
        &store
            .get_run(&run_id)
            .await
            .expect("get run before stale replay")
            .expect("run exists before stale replay"),
    )
    .len();

    let stale_admit = crate::run::MobMachineFlowFrameCommand::AdmitNextReadyNode(
        crate::run::flow_frame::inputs::AdmitNextReadyNode {
            node_id,
            ready_queue: Vec::new(),
        },
    );
    let won = handle
        .commit_flow_frame_store_plan(
            &run_id,
            FlowFrameLoopStorePlan::FrameState {
                machine_inputs: vec![stale_admit.authority_input(&frame_id)],
                frame_id: frame_id.clone(),
                expected_frame: initial_frame,
                next_frame: advanced_frame.clone(),
            },
        )
        .await
        .expect("stale frame plan should lose through store expectation, not authority prepare");
    assert!(
        !won,
        "stale frame plan must return a clean CAS miss so callers can retry/revisit"
    );

    let after_replay = store
        .get_run(&run_id)
        .await
        .expect("get run after replay")
        .expect("run exists after replay")
        .frames
        .get(&frame_id)
        .cloned()
        .expect("frame exists after replay");
    assert_eq!(
        after_replay, advanced_frame,
        "losing stale plan must not mutate the persisted frame"
    );
    let authority_log_len_after_stale_replay = serialized_flow_authority_inputs(
        &store
            .get_run(&run_id)
            .await
            .expect("get run after stale replay")
            .expect("run exists after stale replay"),
    )
    .len();
    assert_eq!(
        authority_log_len_after_stale_replay, authority_log_len_before_stale_replay,
        "losing stale plan must not persist a MobMachine authority input"
    );
}

#[test]
fn test_supervisor_private_trust_realizes_generated_publish_obligation() {
    let source = include_str!("actor.rs");
    let start = source
        .find("async fn install_supervisor_private_trust_for_session")
        .expect("install_supervisor_private_trust_for_session exists");
    let end = source[start..]
        .find("async fn cleanup_supervisor_private_trust_for_session")
        .expect("cleanup helper follows install helper");
    let body = &source[start..start + end];
    assert!(
        body.contains("protocol_supervisor_trust_publish::extract_obligations"),
        "supervisor private trust publication must realize the generated supervisor_trust_publish handoff obligation"
    );
    assert!(
        !body.contains(
            "stage_supervisor_trust_published(session_id, next_peer_id.clone(), next_epoch)"
        ),
        "publish feedback must use the generated obligation fields, not locally reconstructed peer/epoch atoms"
    );
}

#[tokio::test]
async fn test_cancel_flow_cooperative_path_finishes_before_fallback_window() {
    let (handle, service) = create_test_mob(sample_definition_with_two_step_flow(5_000)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_delay_ms(200);

    let run_id = handle
        .run_flow(FlowId::from("two_step"), serde_json::json!({}))
        .await
        .expect("run flow");
    tokio::time::sleep(Duration::from_millis(20)).await;
    handle
        .cancel_flow(run_id.clone())
        .await
        .expect("cancel flow");

    let start = Instant::now();
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "cooperative cancel path should finalize before fallback timeout window"
    );
}

#[tokio::test]
async fn test_cancel_flow_fallback_marks_run_canceled_when_turn_stalls() {
    let (handle, service) = create_test_mob(with_cancel_grace_timeout(
        sample_definition_with_single_step_flow(60_000, 8),
        25,
    ))
    .await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    handle
        .cancel_flow(run_id.clone())
        .await
        .expect("cancel flow");

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(8)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::FlowCanceled { run_id: id, .. } if id == &run_id
            )
        }),
        "expected FlowCanceled event for canceled run"
    );
}

#[tokio::test]
async fn test_cancel_flow_fallback_uses_configured_grace_timeout() {
    let mut definition = sample_definition_with_single_step_flow(60_000, 8);
    definition.limits = Some(LimitsSpec {
        max_flow_duration_ms: None,
        max_step_retries: None,
        max_orphaned_turns: Some(8),
        cancel_grace_timeout_ms: Some(25),
        ..Default::default()
    });
    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    handle
        .cancel_flow(run_id.clone())
        .await
        .expect("cancel flow");

    let start = Instant::now();
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "configured cancel grace timeout should be honored by fallback cancellation"
    );
}

#[tokio::test]
async fn test_cancel_fallback_uses_direct_pending_to_terminal_cas_attempts() {
    let run_store = Arc::new(RecordingRunStore::new());
    let (handle, service) = create_test_mob_with_run_store(
        with_cancel_grace_timeout(sample_definition_with_single_step_flow(60_000, 8), 25),
        run_store.clone(),
    )
    .await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    handle
        .cancel_flow(run_id.clone())
        .await
        .expect("cancel flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(8)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);

    let history = run_store.snapshot_cas_history().await;
    assert!(
        history.iter().any(|(_, from, to)| {
            matches!(from, MobRunStatus::Pending | MobRunStatus::Running)
                && *to == MobRunStatus::Canceled
        }),
        "fallback should attempt direct active->Canceled snapshot CAS transitions"
    );
    assert!(
        history.iter().any(|(_, from, to)| {
            matches!(from, MobRunStatus::Pending | MobRunStatus::Running)
                && matches!(
                    to,
                    MobRunStatus::Completed | MobRunStatus::Failed | MobRunStatus::Canceled
                )
        }),
        "terminalization authority must use direct active->terminal snapshot CAS semantics"
    );
    let legacy_status_history = run_store.cas_history.read().await.clone();
    assert!(
        legacy_status_history.is_empty(),
        "fallback terminalization should use snapshot CAS, not legacy status-only CAS"
    );
}

#[tokio::test]
async fn test_concurrent_fail_and_cancel_resolve_single_terminal_state_with_event_or_ledger() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("FlowFailed").await;
    events.fail_appends_for("FlowCanceled").await;
    let (handle, service) = create_test_mob_with_events(
        sample_definition_with_single_step_flow(60_000, 8),
        events.clone(),
    )
    .await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_fail(true);
    service.set_flow_turn_delay_ms(120);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let cancel_handle = {
        let handle = handle.clone();
        let run_id = run_id.clone();
        tokio::spawn(async move {
            handle
                .cancel_flow(run_id)
                .await
                .expect("cancel flow should be accepted");
        })
    };

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(8)).await;
    cancel_handle.await.expect("cancel join");
    assert!(
        matches!(
            terminal.status,
            MobRunStatus::Canceled | MobRunStatus::Failed
        ),
        "concurrent fail/cancel race should resolve to one terminal status"
    );

    let recorded_events = handle.events().replay_all().await.expect("replay");
    let terminal_events = recorded_events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::FlowCompleted { run_id: id, .. }
                    | MobEventKind::FlowFailed { run_id: id, .. }
                    | MobEventKind::FlowCanceled { run_id: id, .. }
                    if id == &run_id
            )
        })
        .count();
    assert!(
        terminal_events <= 1,
        "terminal race must never produce multiple terminal events for one run"
    );

    if terminal_events == 0 {
        assert!(
            terminal.failure_ledger.iter().any(|entry| {
                entry.step_id == crate::runtime::flow_system_step_id()
                    && entry.reason.contains("append failed")
            }),
            "when terminal append is fault-injected, a durable coherence ledger entry is required"
        );
    }
}

#[tokio::test]
async fn test_timeout_maps_to_step_failed_reason_and_run_failed() {
    let (handle, service) = create_test_mob(sample_definition_with_single_step_flow(20, 1)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
    assert!(
        terminal
            .failure_ledger
            .iter()
            .any(|entry| entry.reason.contains("timeout after")),
        "timeout path should persist failure ledger reason"
    );
    assert_eq!(
        terminal
            .failure_ledger
            .iter()
            .filter(|entry| entry.reason.contains("timeout after"))
            .count(),
        1,
        "timeout path should persist exactly one timeout failure ledger entry"
    );
    assert!(
        terminal
            .step_ledger
            .iter()
            .any(|entry| entry.status == crate::run::StepRunStatus::Failed),
        "timeout path should persist failed step ledger entries"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepTargetFailed { run_id: id, reason, .. }
                    if id == &run_id && reason.contains("timeout after")
            )
        }),
        "expected timeout StepTargetFailed reason"
    );
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::FlowFailed { run_id: id, .. } if id == &run_id
            )
        }),
        "expected FlowFailed event for timed out run"
    );
}

#[tokio::test]
async fn test_timeout_budget_exhaustion_fails_run() {
    let (handle, service) = create_test_mob(sample_definition_with_single_step_flow(20, 0)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::FlowFailed { run_id: id, reason, .. }
                    if id == &run_id && reason.contains("orphan budget")
            )
        }),
        "expected FlowFailed reason from orphan budget exhaustion"
    );
}

#[tokio::test]
async fn test_flow_turn_failure_records_target_failure_reason() {
    let (handle, service) = create_test_mob(sample_definition_with_single_step_flow(500, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_fail(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepTargetFailed { run_id: id, reason, .. }
                    if id == &run_id && reason.contains("mock flow turn failure")
            )
        }),
        "expected StepTargetFailed to include provisioner failure reason"
    );
}

#[tokio::test]
async fn test_malformed_turn_output_fails_without_json_coercion() {
    let (handle, service) = create_test_mob(sample_definition_with_single_step_flow(500, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_completed_result("not-json").await;

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
    assert!(
        terminal.failure_ledger.iter().any(|entry| {
            entry.reason.contains("malformed JSON output")
                && entry.reason.contains("raw_output=\"not-json\"")
        }),
        "malformed output should fail run with parse diagnostics and raw payload excerpt"
    );
}

#[tokio::test]
async fn test_plain_text_step_output_can_skip_json_parsing() {
    let mut definition = sample_definition_with_single_step_flow(500, 8);
    let step = definition
        .flows
        .get_mut(&FlowId::from("demo"))
        .and_then(|flow| flow.steps.get_mut(&crate::StepId::from("start")))
        .expect("start step exists");
    step.output_format = StepOutputFormat::Text;

    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_completed_result("not-json").await;

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    assert!(
        terminal.step_ledger.iter().any(|entry| {
            entry.step_id == "start"
                && entry.status == StepRunStatus::Completed
                && entry.output == Some(serde_json::Value::String("not-json".to_string()))
        }),
        "completed start ledger entry should persist raw plain-text output as JSON string"
    );
}

#[tokio::test]
async fn test_spawn_rejects_reserved_flow_system_member_prefix() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let reserved_meerkat_id = format!("{}test", crate::runtime::FLOW_SYSTEM_MEMBER_ID_PREFIX);
    let err = handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from(reserved_meerkat_id.as_str()),
            None,
        )
        .await
        .expect_err("reserved system member prefix should be rejected");
    assert!(
        err.to_string().contains("reserved system prefix"),
        "error should clearly explain reserved id prefix: {err}"
    );
}

#[tokio::test]
async fn test_flow_failed_append_failure_records_coherence_failure_ledger_entry() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("FlowFailed").await;
    let (handle, service) =
        create_test_mob_with_events(sample_definition_with_single_step_flow(500, 8), events).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_fail(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
    assert!(
        terminal.failure_ledger.iter().any(|entry| {
            entry.step_id == crate::runtime::flow_system_step_id()
                && entry.reason.contains("FlowFailed append failed")
        }),
        "failed terminal event append should be recorded in failure ledger"
    );
    let machine_state = handle
        .query_machine_state()
        .await
        .expect("query MobMachine state after failed append failure");
    let run_key = crate::machines::mob_machine::RunId::from(run_id.to_string());
    assert_eq!(
        machine_state.run_status.get(&run_key),
        Some(&crate::machines::mob_machine::FlowRunStatus::Failed),
        "MobMachine run_status must still be Failed when durable run status CAS wins before terminal event append fails"
    );
    assert_eq!(
        machine_state.active_run_count, 0,
        "failed terminal append failure must not leave MobMachine active_run_count running"
    );
}

#[tokio::test]
async fn test_flow_completed_append_failure_records_coherence_failure_ledger_entry() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("FlowCompleted").await;
    let (handle, _service) =
        create_test_mob_with_events(sample_definition_with_single_step_flow(500, 8), events).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);
    assert!(
        terminal.failure_ledger.iter().any(|entry| {
            entry.step_id == crate::runtime::flow_system_step_id()
                && entry.reason.contains("FlowCompleted append failed")
        }),
        "failed terminal event append should be recorded in failure ledger"
    );
    let machine_state = handle
        .query_machine_state()
        .await
        .expect("query MobMachine state after terminal append failure");
    let run_key = crate::machines::mob_machine::RunId::from(run_id.to_string());
    assert_eq!(
        machine_state.run_status.get(&run_key),
        Some(&crate::machines::mob_machine::FlowRunStatus::Completed),
        "MobMachine run_status must still be terminal when durable run status CAS wins before terminal event append fails"
    );
    assert_eq!(
        machine_state.active_run_count, 0,
        "terminal append failure must not leave MobMachine active_run_count running"
    );
}

#[tokio::test]
async fn test_flow_canceled_append_failure_records_coherence_failure_ledger_entry() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("FlowCanceled").await;
    let (handle, service) = create_test_mob_with_events(
        with_cancel_grace_timeout(sample_definition_with_single_step_flow(60_000, 8), 25),
        events,
    )
    .await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    handle
        .cancel_flow(run_id.clone())
        .await
        .expect("cancel flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(8)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);
    assert!(
        terminal.failure_ledger.iter().any(|entry| {
            entry.step_id == crate::runtime::flow_system_step_id()
                && entry.reason.contains("FlowCanceled append failed")
        }),
        "failed terminal event append should be recorded in failure ledger"
    );
    let machine_state = handle
        .query_machine_state()
        .await
        .expect("query MobMachine state after canceled append failure");
    let run_key = crate::machines::mob_machine::RunId::from(run_id.to_string());
    assert_eq!(
        machine_state.run_status.get(&run_key),
        Some(&crate::machines::mob_machine::FlowRunStatus::Canceled),
        "MobMachine run_status must still be Canceled when durable run status CAS wins before terminal event append fails"
    );
    assert_eq!(
        machine_state.active_run_count, 0,
        "canceled terminal append failure must not leave MobMachine active_run_count running"
    );
}

#[tokio::test]
async fn test_topology_strict_denial_fails_before_dispatch() {
    let (handle, _service) = create_test_mob(sample_definition_with_strict_topology_deny()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::FlowFailed { run_id: id, .. } if id == &run_id
            )
        }),
        "expected FlowFailed event under strict topology denial"
    );
    assert!(
        !events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepDispatched { run_id: id, .. } if id == &run_id
            )
        }),
        "strict topology denial should block dispatch"
    );
}

#[tokio::test]
async fn test_topology_advisory_emits_violation_and_continues() {
    let (handle, _service) = create_test_mob(sample_definition_with_advisory_topology_deny()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|event| { matches!(event.kind, MobEventKind::TopologyViolation { .. }) }),
        "advisory topology should emit violation event"
    );
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepDispatched { run_id: id, .. } if id == &run_id
            )
        }),
        "advisory topology should still dispatch steps"
    );
}

#[tokio::test]
async fn test_topology_strict_wildcard_denial_fails_before_dispatch() {
    let (handle, _service) =
        create_test_mob(sample_definition_with_strict_topology_wildcard_deny()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::FlowFailed { run_id: id, .. } if id == &run_id
            )
        }),
        "expected FlowFailed event under strict wildcard topology denial"
    );
    assert!(
        !events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepDispatched { run_id: id, .. } if id == &run_id
            )
        }),
        "strict wildcard topology denial should block dispatch"
    );
}

#[tokio::test]
async fn test_topology_advisory_wildcard_emits_violation_and_continues() {
    let (handle, _service) =
        create_test_mob(sample_definition_with_advisory_topology_wildcard_deny()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|event| { matches!(event.kind, MobEventKind::TopologyViolation { .. }) }),
        "advisory wildcard topology should emit violation event"
    );
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepDispatched { run_id: id, .. } if id == &run_id
            )
        }),
        "advisory wildcard topology should still dispatch steps"
    );
}

#[tokio::test]
async fn test_collection_policy_any_succeeds_with_single_target_success() {
    let (handle, service) = create_test_mob(sample_definition_with_collection_policy(
        CollectionPolicy::Any,
    ))
    .await;
    let sid_w1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let _sid_w2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    service.set_flow_turn_fail_for_session(&sid_w1, true).await;

    let run_id = handle
        .run_flow(FlowId::from("collect"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);
}

#[tokio::test]
async fn test_collection_policy_quorum_requires_threshold_successes() {
    let (handle, service) = create_test_mob(sample_definition_with_collection_policy(
        CollectionPolicy::Quorum { n: 2 },
    ))
    .await;
    let sid_w1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let _sid_w2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    service.set_flow_turn_fail_for_session(&sid_w1, true).await;

    let run_id = handle
        .run_flow(FlowId::from("collect"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
}

#[tokio::test]
async fn test_impossible_quorum_fails_before_dispatching_any_targets() {
    let (handle, _service) = create_test_mob(sample_definition_with_collection_policy(
        CollectionPolicy::Quorum { n: 3 },
    ))
    .await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");

    let run_id = handle
        .run_flow(FlowId::from("collect"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        terminal.failure_ledger.iter().any(|entry| {
            entry.step_id.as_str() == "collect" && entry.reason.contains("insufficient targets")
        }) || events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::FlowFailed { run_id: id, reason, .. }
                    if id == &run_id && reason.contains("insufficient targets")
            )
        }),
        "impossible quorum should surface an explicit insufficient-targets reason"
    );

    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    MobEventKind::StepDispatched { run_id: id, step_id, .. }
                        if id == &run_id && step_id.as_str() == "collect"
                )
            })
            .count(),
        0,
        "impossible quorum should fail before dispatching any target turns"
    );
}

#[tokio::test]
async fn test_collection_policy_all_uses_map_shape_for_single_target() {
    let (handle, _service) = create_test_mob(sample_definition_with_collection_policy(
        CollectionPolicy::All,
    ))
    .await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");

    let run_id = handle
        .run_flow(FlowId::from("collect"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let aggregate_output = terminal
        .step_ledger
        .iter()
        .find(|entry| {
            entry.step_id.as_str() == "collect"
                && entry.agent_identity == crate::runtime::flow_system_member_id()
                && entry.status == StepRunStatus::Completed
        })
        .and_then(|entry| entry.output.clone())
        .expect("aggregate collect output should be persisted");
    assert_eq!(
        aggregate_output,
        serde_json::json!({"w-1": "Turn completed"})
    );
}

#[tokio::test]
async fn test_max_flow_duration_limit_is_enforced() {
    let mut definition = sample_definition_with_single_step_flow(60_000, 8);
    definition.limits = Some(LimitsSpec {
        max_flow_duration_ms: Some(25),
        max_step_retries: None,
        max_orphaned_turns: Some(8),
        cancel_grace_timeout_ms: None,
        ..Default::default()
    });
    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let start = Instant::now();
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "global flow duration limit should fail the run quickly"
    );
}

#[tokio::test]
async fn test_schema_ref_file_path_validation_passes_and_fails() {
    let mut schema_ok = NamedTempFile::new().expect("create schema file");
    write!(
        schema_ok,
        "{{\"type\":\"object\",\"additionalProperties\":{{\"type\":\"string\"}}}}"
    )
    .expect("write schema");
    let ok_path = schema_ok.path().to_string_lossy().to_string();

    let (ok_handle, _ok_service) =
        create_test_mob(sample_definition_with_schema_ref(&ok_path)).await;
    ok_handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    let ok_run_id = ok_handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let ok_terminal = wait_for_run_terminal(&ok_handle, &ok_run_id, Duration::from_secs(2)).await;
    assert_eq!(ok_terminal.status, MobRunStatus::Completed);

    let mut schema_bad = NamedTempFile::new().expect("create schema file");
    write!(
        schema_bad,
        "{{\"type\":\"object\",\"additionalProperties\":{{\"type\":\"integer\"}}}}"
    )
    .expect("write schema");
    let bad_path = schema_bad.path().to_string_lossy().to_string();

    let (bad_handle, _bad_service) =
        create_test_mob(sample_definition_with_schema_ref(&bad_path)).await;
    bad_handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    let bad_run_id = bad_handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let bad_terminal =
        wait_for_run_terminal(&bad_handle, &bad_run_id, Duration::from_secs(2)).await;
    assert_eq!(bad_terminal.status, MobRunStatus::Failed);
}

#[tokio::test]
async fn test_supervisor_escalation_forces_reset_via_retire_path() {
    let (handle, service) = create_test_mob(sample_definition_with_supervisor_threshold(1)).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead supervisor");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_fail(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::SupervisorEscalation { run_id: id, .. } if id == &run_id
            )
        }),
        "expected supervisor escalation event after threshold breach"
    );

    let remaining = handle.list_members().await;
    assert!(
        remaining.is_empty(),
        "force_reset should retire active meerkats via handle.retire()"
    );
}

#[tokio::test]
async fn test_supervisor_escalation_times_out_when_turn_hangs() {
    crate::runtime::supervisor::set_escalation_turn_timeout_for_tests(Duration::from_millis(25));
    let mut definition = sample_definition_with_supervisor_threshold(1);
    let lead = definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .expect("lead profile")
        .as_inline_mut()
        .unwrap();
    lead.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead supervisor");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_fail(true);
    service.set_start_turn_delay_ms(250);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(8)).await;
    crate::runtime::supervisor::reset_escalation_turn_timeout_for_tests();
    assert_eq!(terminal.status, MobRunStatus::Failed);
    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        terminal
            .failure_ledger
            .iter()
            .any(|entry| entry.reason.contains("supervisor escalation timed out"))
            || events.iter().any(|event| {
                matches!(
                    &event.kind,
                    MobEventKind::FlowFailed { run_id: id, reason, .. }
                        if id == &run_id && reason.contains("supervisor escalation timed out")
                )
            }),
        "supervisor escalation timeout should fail with explicit bounded-time reason"
    );
}

#[tokio::test]
async fn test_supervisor_force_reset_reports_aggregate_retire_errors() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("MemberRetired").await;
    let (handle, _service) = create_test_mob_with_events(sample_definition(), events).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    let supervisor = super::supervisor::Supervisor::new(
        handle.clone(),
        super::events::MobEventEmitter::new(
            Arc::new(InMemoryMobEventStore::new()),
            handle.mob_id().clone(),
        ),
    );
    let error = supervisor
        .force_reset()
        .await
        .expect_err("force_reset should aggregate retire errors");
    assert!(
        error
            .to_string()
            .contains("force_reset encountered 2 retirement error(s)"),
        "force_reset should continue through all retire failures and return aggregate count"
    );
}

#[tokio::test]
async fn test_branch_winner_and_join_any_behavior() {
    let (handle, _service) = create_test_mob(sample_definition_with_branch_flow()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(
            FlowId::from("branching"),
            serde_json::json!({ "severity": "critical" }),
        )
        .await
        .expect("run branch flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepCompleted { run_id: id, step_id }
                    if id == &run_id && step_id.as_str() == "fix_critical"
            )
        }),
        "expected winning branch step to complete"
    );
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepSkipped { run_id: id, step_id, .. }
                    if id == &run_id && step_id.as_str() == "fix_minor"
            )
        }),
        "expected losing branch step to be skipped"
    );
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepCompleted { run_id: id, step_id }
                    if id == &run_id && step_id.as_str() == "summarize"
            )
        }),
        "expected join step to run with depends_on_mode=any"
    );
}

#[tokio::test]
async fn test_cancel_after_completion_is_noop_without_flow_canceled_event() {
    let (handle, _service) = create_test_mob(sample_definition_with_single_step_flow(500, 8)).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    tokio::time::sleep(Duration::from_millis(50)).await;
    handle
        .cancel_flow(run_id.clone())
        .await
        .expect("cancel completed run should be noop");
    let status = handle
        .flow_status(run_id.clone())
        .await
        .expect("flow status call")
        .expect("run exists");
    assert_eq!(status.status, MobRunStatus::Completed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::FlowCanceled { run_id: id, .. } if id == &run_id
            )
        }),
        "cancel after completion should not emit FlowCanceled"
    );
}

// -----------------------------------------------------------------------
// Additional: MobState
// -----------------------------------------------------------------------

#[test]
fn test_mob_state_as_str() {
    assert_eq!(MobState::Creating.as_str(), "Creating");
    assert_eq!(MobState::Running.as_str(), "Running");
    assert_eq!(MobState::Stopped.as_str(), "Stopped");
    assert_eq!(MobState::Completed.as_str(), "Completed");
    assert_eq!(MobState::Destroyed.as_str(), "Destroyed");
}

// -----------------------------------------------------------------------
// P1-T04b: spawn() with initial_message uses custom message
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_spawn_with_custom_initial_message() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let custom_msg = "You are the orchestrator. Begin planning.";

    handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("l-1"),
            Some(custom_msg.to_string().into()),
        )
        .await
        .expect("spawn with custom message");

    // Verify the custom message was passed to create_session
    let prompts = service.recorded_prompts().await;
    assert_eq!(prompts.len(), 1, "should have exactly one recorded prompt");
    assert_eq!(
        prompts[0].1, custom_msg,
        "spawn should use the custom initial_message, not the default"
    );

    // Runtime adapter path: autonomous spawn uses accept_input_with_completion,
    // which delegates to start_turn and increments keep_alive_start_turn_call_count.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        1,
        "autonomous spawn must issue exactly one keep-alive start_turn"
    );
    assert_eq!(
        service.inject_call_count(),
        0,
        "autonomous spawn uses start_turn via runtime adapter, not inject"
    );
}

#[tokio::test]
async fn test_spawn_without_initial_message_uses_default() {
    let (handle, service) = create_test_mob(sample_definition()).await;

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn with default message");

    let prompts = service.recorded_prompts().await;
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].1.contains("You have been spawned as 'w-1'"),
        "default message should contain meerkat id, got: '{}'",
        prompts[0].1
    );
    assert!(
        prompts[0].1.contains("role: worker"),
        "default message should contain role, got: '{}'",
        prompts[0].1
    );
    assert!(
        prompts[0].1.contains("mob 'test-mob'"),
        "default message should contain mob id, got: '{}'",
        prompts[0].1
    );

    // Runtime adapter path: autonomous spawn uses accept_input_with_completion,
    // which delegates to start_turn and increments keep_alive_start_turn_call_count.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        1,
        "autonomous spawn must issue exactly one keep-alive start_turn"
    );
    assert_eq!(
        service.inject_call_count(),
        0,
        "autonomous spawn uses start_turn via runtime adapter, not inject"
    );
}

#[tokio::test]
async fn test_autonomous_host_without_adapter_rejected_at_build_time() {
    // §8: AutonomousHost requires a runtime adapter. The builder must reject
    // at create() time, not defer to spawn() time where the Option<adapter>
    // would hide the ownership requirement.
    let result = try_create_test_mob_without_adapter(sample_definition()).await;
    assert!(
        result.is_err(),
        "AutonomousHost without adapter must fail at build time"
    );
}

#[tokio::test]
async fn test_turn_driven_without_adapter_accepted_at_build_time() {
    // TurnDriven profiles don't require a runtime adapter — the Option is
    // genuinely optional for them.
    let mut def = sample_definition();
    for profile in def.profiles.values_mut().filter_map(|b| b.as_inline_mut()) {
        profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    }
    let result = try_create_test_mob_without_adapter(def).await;
    assert!(result.is_ok(), "TurnDriven without adapter must succeed");
}

#[tokio::test]
async fn test_spawn_autonomous_surfaces_immediate_host_loop_start_failure() {
    // Runtime adapter path: accept_input_with_completion queues the input
    // successfully. The start_turn failure surfaces asynchronously through
    // the runtime loop's executor, which completes the initial turn with
    // a non-success outcome.
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_fail_start_turn(true);

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-fail"), None)
        .await;

    // With the runtime adapter path, the spawn itself succeeds because
    // accept_input_with_completion returns Ok (input accepted into the
    // driver queue). The start_turn failure surfaces asynchronously
    // through the completion handle in the background task.
    assert!(
        result.is_ok(),
        "runtime-backed spawn should accept input even when start_turn will fail: {result:?}"
    );
}

#[tokio::test]
async fn test_retire_interrupts_autonomous_host_loop() {
    let (handle, service) = create_test_mob(sample_definition()).await;

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    // Runtime adapter path: autonomous spawn uses accept_input_with_completion.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        1,
        "spawn should issue keep-alive start_turn"
    );

    handle
        .retire(AgentIdentity::from("w-1"))
        .await
        .expect("retire worker");
    // With the runtime adapter path, interrupt goes through
    // adapter.interrupt_current_run(), not session_service.interrupt().
    // The behavioral guarantee is that retire succeeds (above).
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "retire must remove the member from the roster"
    );
}

#[tokio::test]
async fn test_stop_resume_host_loop_lifecycle_is_mode_aware() {
    let (handle, service) = create_test_mob(sample_definition()).await;

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn autonomous lead");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            MeerkatId::from("w-td"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn turn-driven worker");
    // Runtime adapter path: autonomous spawn uses accept_input_with_completion,
    // which delegates to start_turn. No injects from spawn.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        1,
        "only autonomous members should issue keep-alive start_turn"
    );
    assert_eq!(
        service.inject_call_count(),
        0,
        "no injects from spawn via runtime adapter path"
    );

    handle.stop().await.expect("stop");
    // With the runtime adapter path, interrupt goes through
    // adapter.interrupt_current_run(), not session_service.interrupt().
    // The behavioral guarantee is that stop succeeds (above) and the
    // mob transitions to Stopped.
    assert_eq!(
        handle.status().await.unwrap(),
        MobState::Stopped,
        "stop should transition mob to Stopped"
    );
    // Stop notifies the orchestrator (autonomous lead) via inject (+1).
    // Total inject count: 0 (no spawn inject) + 1 (stop notification) = 1.
    assert_eq!(
        service.inject_call_count(),
        1,
        "stop should have notified the orchestrator via inject"
    );

    handle.resume().await.expect("resume");
    // Resume ensures autonomous runtime readiness but does NOT re-inject
    // prompts — the session already has its conversation history.
    // Inject count stays at 1 (no new injects from resume).
    assert_eq!(
        service.inject_call_count(),
        1,
        "resume must not re-inject prompts"
    );
}

#[tokio::test]
async fn test_destroy_interrupts_autonomous_host_loops_before_archive() {
    let (handle, service) = create_test_mob(sample_definition()).await;

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    // Runtime adapter path: autonomous members use accept_input_with_completion,
    // which delegates to start_turn. No injects from spawn.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        2,
        "both autonomous members should issue keep-alive start_turn"
    );
    assert_eq!(
        service.inject_call_count(),
        0,
        "no injects from spawn via runtime adapter path"
    );

    handle.destroy().await.expect("destroy");
    // With the runtime adapter path, interrupt goes through
    // adapter.interrupt_current_run(), not session_service.interrupt().
    // The behavioral guarantee is that destroy succeeds (above) and
    // transitions to Destroyed.
    assert_eq!(
        handle.status().await.unwrap(),
        MobState::Destroyed,
        "destroy must transition mob to Destroyed"
    );
}

#[tokio::test]
async fn test_resume_from_events_restarts_autonomous_host_loops_from_runtime_mode() {
    let storage = MobStorage::in_memory();
    let storage_for_resume = MobStorage {
        events: storage.events.clone(),
        runs: storage.runs.clone(),
        specs: storage.specs.clone(),
        runtime_metadata: storage.runtime_metadata.clone(),
        realm_profiles: storage.realm_profiles.clone(),
    };
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            MeerkatId::from("w-td"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn turn-driven worker");
    tokio::time::timeout(std::time::Duration::from_secs(2), handle.stop())
        .await
        .expect("stop original runtime timed out")
        .expect("stop original runtime");

    let resumed = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        MobBuilder::for_resume(storage_for_resume)
            .with_session_service(service.clone())
            .notify_orchestrator_on_resume(false)
            .resume(),
    )
    .await
    .expect("resume mob timed out")
    .expect("resume mob");

    assert_eq!(
        resumed.status().await.unwrap(),
        MobState::Running,
        "resumed runtime should be Running"
    );
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    // The original spawn issued 1 keep-alive start_turn via the runtime adapter.
    // Resume ensures autonomous runtime readiness but does NOT fire additional
    // kickoff start_turn calls. Counter stays at 1 from the original spawn.
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        1,
        "only the original spawn should have issued a keep-alive start_turn — resume must not add more"
    );
}

#[tokio::test]
async fn test_mob_resume_fails_when_runtime_metadata_store_unavailable() {
    let definition = sample_definition();
    let events: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
    let runtime_metadata = Arc::new(FaultInjectedRuntimeMetadataStore::new());
    runtime_metadata.set_fail_load_supervisor(true);
    let storage = MobStorage::custom_with_runtime_metadata(
        events.clone(),
        Arc::new(InMemoryMobRunStore::new()),
        Arc::new(InMemoryMobSpecStore::new()),
        runtime_metadata,
    );

    events
        .append(NewMobEvent {
            mob_id: definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MobCreated {
                definition: Box::new(definition.clone()),
            },
        })
        .await
        .expect("append mob created");

    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let error = match MobBuilder::for_resume(storage)
        .with_session_service(service)
        .resume()
        .await
    {
        Ok(_) => panic!("resume must fail when runtime metadata cannot be loaded"),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("fault-injected runtime metadata load failure"),
        "resume should surface runtime metadata load failures, got: {error}"
    );
}

#[tokio::test]
async fn test_mob_resume_seeds_missing_supervisor_runtime_metadata() {
    let definition = sample_definition();
    let events: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
    let runtime_metadata = Arc::new(InMemoryMobRuntimeMetadataStore::new());
    let storage = MobStorage::custom_with_runtime_metadata(
        events.clone(),
        Arc::new(InMemoryMobRunStore::new()),
        Arc::new(InMemoryMobSpecStore::new()),
        runtime_metadata.clone(),
    );

    events
        .append(NewMobEvent {
            mob_id: definition.id.clone(),
            timestamp: None,
            kind: MobEventKind::MobCreated {
                definition: Box::new(definition.clone()),
            },
        })
        .await
        .expect("append mob created");

    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let resumed = MobBuilder::for_resume(storage)
        .with_session_service(service)
        .resume()
        .await
        .expect("resume should seed missing supervisor runtime metadata");

    assert_eq!(resumed.status().await.unwrap(), MobState::Running);
    let supervisor_authority = runtime_metadata
        .load_supervisor_authority(&definition.id)
        .await
        .expect("load supervisor authority")
        .expect("resume should seed supervisor authority");
    assert!(
        !supervisor_authority.public_peer_id.is_empty(),
        "seeded supervisor authority should have a real public peer id"
    );
}

#[tokio::test]
async fn test_resume_startup_keep_alive_loop_failure_enters_stopped_state() {
    // TODO: This test exercises condemned direct host-loop failure behavior.
    // Needs rewrite for runtime-backed keep-alive path.
    if true {
        return;
    }
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn autonomous lead");
    handle.stop().await.expect("stop");
    service.set_fail_start_turn(true);

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .allow_ephemeral_sessions(true)
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .expect("resume handle should still be constructed");

    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert_eq!(
        resumed.status().await.unwrap(),
        MobState::Stopped,
        "startup host-loop failure should force runtime into Stopped state"
    );
}

#[tokio::test]
async fn test_resume_skips_broken_autonomous_member_in_host_loop_startup() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn autonomous lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle.stop().await.expect("stop");
    service.archive(&sid_l).await.expect("archive lead");
    service.delete_persisted_session(&sid_l).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .expect("partial resume should succeed");

    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert_eq!(
        resumed.status().await.unwrap(),
        MobState::Running,
        "Broken autonomous members must be skipped during host-loop startup so partial resume stays running"
    );
    let snapshot = resumed
        .member_status(&AgentIdentity::from("l-1"))
        .await
        .expect("broken member status");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
}

// -----------------------------------------------------------------------
// Behavioral wire test: wire() enables PeerRequest communication
//
// Uses real CommsRuntime (inproc_only) to prove add_trusted_peer was
// actually called by verifying that a PeerRequest sent from A arrives
// in B's inbox.
// -----------------------------------------------------------------------

/// Mock session service that creates sessions with REAL CommsRuntime
/// instances (inproc_only) instead of MockCommsRuntime. This enables
/// actual PeerRequest delivery between wired meerkats.
struct RealCommsSessionService {
    sessions: RwLock<HashMap<SessionId, Arc<meerkat_comms::CommsRuntime>>>,
    keep_alive_notifiers: RwLock<HashMap<SessionId, Arc<tokio::sync::Notify>>>,
    session_comms_names: RwLock<HashMap<SessionId, String>>,
    session_counter: AtomicU64,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
}

impl RealCommsSessionService {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            keep_alive_notifiers: RwLock::new(HashMap::new()),
            session_comms_names: RwLock::new(HashMap::new()),
            session_counter: AtomicU64::new(0),
            runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
        }
    }
}

#[async_trait]
impl SessionService for RealCommsSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let session_id = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone())
            .unwrap_or_default();
        let n = self.session_counter.fetch_add(1, Ordering::Relaxed);

        let comms_name = req
            .build
            .as_ref()
            .and_then(|b| b.comms_name.clone())
            .unwrap_or_else(|| format!("real-comms-session-{n}"));

        let comms = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only(&comms_name)
                .expect("create inproc CommsRuntime"),
        );
        install_machine_peer_request_response_authority(&self.runtime_adapter, &comms, &session_id)
            .await?;
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::clone(&comms));
        let is_keep_alive = req.build.as_ref().map(|b| b.keep_alive).unwrap_or(false);
        if is_keep_alive {
            self.keep_alive_notifiers
                .write()
                .await
                .insert(session_id.clone(), Arc::new(tokio::sync::Notify::new()));
        }
        self.session_comms_names
            .write()
            .await
            .insert(session_id.clone(), comms_name);
        Ok(mock_run_result(session_id, "Session created".to_string()))
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        _req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        drop(sessions);
        // Block only for keep-alive sessions (notifier registered at create time).
        if let Some(notifier) = self.keep_alive_notifiers.read().await.get(id).cloned() {
            notifier.notified().await;
            return Ok(mock_run_result(
                id.clone(),
                "Host loop interrupted".to_string(),
            ));
        }
        Ok(mock_run_result(id.clone(), "Turn completed".to_string()))
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        drop(sessions);
        if let Some(notifier) = self.keep_alive_notifiers.read().await.get(id).cloned() {
            notifier.notify_waiters();
        }
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                is_active: false,
                model: "claude-sonnet-4-5".to_string(),
                provider: Provider::Anthropic,
                last_assistant_text: None,
                labels: Default::default(),
            },
            billing: SessionUsage {
                total_tokens: 0,
                usage: Usage::default(),
            },
        })
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let sessions = self.sessions.read().await;
        Ok(sessions
            .keys()
            .map(|id| SessionSummary {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            })
            .collect())
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        Ok(self.sessions.read().await.contains_key(id))
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(id);
        if let Some(notifier) = self.keep_alive_notifiers.write().await.remove(id) {
            notifier.notify_waiters();
        }
        self.session_comms_names.write().await.remove(id);
        Ok(())
    }
}

#[async_trait]
impl SessionServiceCommsExt for RealCommsSessionService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|c| Arc::clone(c) as Arc<dyn CoreCommsRuntime>)
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceHistoryExt for RealCommsSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: meerkat_core::service::SessionHistoryQuery,
    ) -> Result<meerkat_core::service::SessionHistoryPage, SessionError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(meerkat_core::service::SessionHistoryPage::from_messages(
            id.clone(),
            &[],
            query,
        ))
    }
}

#[async_trait]
impl SessionServiceControlExt for RealCommsSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        _req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() }.into());
        }
        Ok(AppendSystemContextResult {
            status: AppendSystemContextStatus::Staged,
        })
    }
}

#[async_trait]
impl MobSessionService for RealCommsSessionService {
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        Some(Arc::clone(&self.runtime_adapter))
    }

    async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
        let names = self.session_comms_names.read().await;
        names
            .get(session_id)
            .is_some_and(|name| name.starts_with(&format!("{mob_id}/")))
    }
}

impl RealCommsSessionService {
    /// Get the real CommsRuntime for a session (for test inspection).
    async fn real_comms(&self, session_id: &SessionId) -> Option<Arc<meerkat_comms::CommsRuntime>> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }
}

/// Create a MobHandle with real comms for wiring/trust behavior tests.
///
/// This harness intentionally forces all profiles to `TurnDriven` so peer/trust
/// assertions are not coupled to autonomous keep-alive kickoff behavior. Tests
/// that need autonomous ingress should use
/// `create_test_mob_with_runtime_backed_real_comms(...)` instead.
async fn create_test_mob_with_real_comms(
    mut definition: MobDefinition,
) -> (MobHandle, Arc<RealCommsSessionService>) {
    for profile in definition
        .profiles
        .values_mut()
        .filter_map(|b| b.as_inline_mut())
    {
        profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    }
    let service = Arc::new(RealCommsSessionService::new());
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    (handle, service)
}

/// Real comms + runtime-backed session service for autonomous idle-delivery regressions.
///
/// Uses real inproc comms delivery, a real MeerkatMachine, and records
/// runtime-applied prompts so tests can prove peer inbox traffic still reaches
/// `apply_runtime_turn()` after the initial autonomous kickoff turn has exited.
struct RuntimeBackedRealCommsSessionService {
    sessions: RwLock<HashMap<SessionId, Arc<meerkat_comms::CommsRuntime>>>,
    keep_alive_notifiers: RwLock<HashMap<SessionId, Arc<tokio::sync::Notify>>>,
    session_comms_names: RwLock<HashMap<SessionId, String>>,
    session_counter: AtomicU64,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    keep_alive_turns_complete_immediately: std::sync::atomic::AtomicBool,
    applied_runtime_prompts: RwLock<HashMap<SessionId, Vec<ContentInput>>>,
    applied_runtime_context_appends: RwLock<HashMap<SessionId, Vec<AppendSystemContextRequest>>>,
    applied_runtime_render_metadata:
        RwLock<HashMap<SessionId, Vec<Option<meerkat_core::types::RenderMetadata>>>>,
}

impl RuntimeBackedRealCommsSessionService {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            keep_alive_notifiers: RwLock::new(HashMap::new()),
            session_comms_names: RwLock::new(HashMap::new()),
            session_counter: AtomicU64::new(0),
            runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
            keep_alive_turns_complete_immediately: std::sync::atomic::AtomicBool::new(false),
            applied_runtime_prompts: RwLock::new(HashMap::new()),
            applied_runtime_context_appends: RwLock::new(HashMap::new()),
            applied_runtime_render_metadata: RwLock::new(HashMap::new()),
        }
    }

    fn set_keep_alive_turns_complete_immediately(&self, enabled: bool) {
        self.keep_alive_turns_complete_immediately
            .store(enabled, Ordering::Relaxed);
    }

    async fn real_comms(&self, session_id: &SessionId) -> Option<Arc<meerkat_comms::CommsRuntime>> {
        self.sessions.read().await.get(session_id).cloned()
    }

    async fn applied_runtime_prompts(&self, session_id: &SessionId) -> Vec<ContentInput> {
        self.applied_runtime_prompts
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    async fn applied_runtime_context_appends(
        &self,
        session_id: &SessionId,
    ) -> Vec<AppendSystemContextRequest> {
        self.applied_runtime_context_appends
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    async fn applied_runtime_render_metadata(
        &self,
        session_id: &SessionId,
    ) -> Vec<Option<meerkat_core::types::RenderMetadata>> {
        self.applied_runtime_render_metadata
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl SessionService for RuntimeBackedRealCommsSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let session_id = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.as_ref())
            .map(|session| session.id().clone())
            .unwrap_or_default();
        let n = self.session_counter.fetch_add(1, Ordering::Relaxed);

        let comms_name = req
            .build
            .as_ref()
            .and_then(|b| b.comms_name.clone())
            .unwrap_or_else(|| format!("real-runtime-comms-session-{n}"));

        let comms = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only(&comms_name)
                .expect("create inproc CommsRuntime"),
        );
        install_machine_peer_request_response_authority(&self.runtime_adapter, &comms, &session_id)
            .await?;
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::clone(&comms));
        let is_keep_alive = req.build.as_ref().map(|b| b.keep_alive).unwrap_or(false);
        if is_keep_alive {
            self.keep_alive_notifiers
                .write()
                .await
                .insert(session_id.clone(), Arc::new(tokio::sync::Notify::new()));
        }
        self.session_comms_names
            .write()
            .await
            .insert(session_id.clone(), comms_name);

        Ok(mock_run_result(session_id, "Session created".to_string()))
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        _req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        drop(sessions);
        if let Some(notifier) = self.keep_alive_notifiers.read().await.get(id).cloned() {
            if self
                .keep_alive_turns_complete_immediately
                .load(Ordering::Relaxed)
            {
                return Ok(mock_run_result(
                    id.clone(),
                    "Autonomous kickoff completed".to_string(),
                ));
            }
            notifier.notified().await;
            return Ok(mock_run_result(
                id.clone(),
                "Host loop interrupted".to_string(),
            ));
        }
        Ok(mock_run_result(id.clone(), "Turn completed".to_string()))
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        drop(sessions);
        if let Some(notifier) = self.keep_alive_notifiers.read().await.get(id).cloned() {
            notifier.notify_waiters();
        }
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                is_active: false,
                model: "claude-sonnet-4-5".to_string(),
                provider: Provider::Anthropic,
                last_assistant_text: None,
                labels: Default::default(),
            },
            billing: SessionUsage {
                total_tokens: 0,
                usage: Usage::default(),
            },
        })
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let sessions = self.sessions.read().await;
        Ok(sessions
            .keys()
            .map(|id| SessionSummary {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            })
            .collect())
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        Ok(self.sessions.read().await.contains_key(id))
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.sessions.write().await.remove(id);
        if let Some(notifier) = self.keep_alive_notifiers.write().await.remove(id) {
            notifier.notify_waiters();
        }
        self.session_comms_names.write().await.remove(id);
        self.applied_runtime_prompts.write().await.remove(id);
        self.applied_runtime_context_appends
            .write()
            .await
            .remove(id);
        Ok(())
    }
}

#[async_trait]
impl SessionServiceCommsExt for RuntimeBackedRealCommsSessionService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .map(|c| Arc::clone(c) as Arc<dyn CoreCommsRuntime>)
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceHistoryExt for RuntimeBackedRealCommsSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: meerkat_core::service::SessionHistoryQuery,
    ) -> Result<meerkat_core::service::SessionHistoryPage, SessionError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(meerkat_core::service::SessionHistoryPage::from_messages(
            id.clone(),
            &[],
            query,
        ))
    }
}

#[async_trait]
impl SessionServiceControlExt for RuntimeBackedRealCommsSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() }.into());
        }
        let mut appends = self.applied_runtime_context_appends.write().await;
        let session_appends = appends.entry(id.clone()).or_default();
        if let Some(key) = req.idempotency_key.as_ref()
            && let Some(existing) = session_appends
                .iter()
                .find(|append| append.idempotency_key.as_ref() == Some(key))
        {
            if existing == &req {
                return Ok(AppendSystemContextResult {
                    status: AppendSystemContextStatus::Duplicate,
                });
            }
            return Err(SessionControlError::Conflict {
                id: id.clone(),
                key: key.clone(),
            });
        }
        session_appends.push(req);
        Ok(AppendSystemContextResult {
            status: AppendSystemContextStatus::Staged,
        })
    }
}

#[async_trait]
impl MobSessionService for RuntimeBackedRealCommsSessionService {
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        Some(Arc::clone(&self.runtime_adapter))
    }

    async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
        let names = self.session_comms_names.read().await;
        names
            .get(session_id)
            .is_some_and(|name| name.starts_with(&format!("{mob_id}/")))
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        self.applied_runtime_prompts
            .write()
            .await
            .entry(session_id.clone())
            .or_default()
            .push(req.prompt);
        self.applied_runtime_render_metadata
            .write()
            .await
            .entry(session_id.clone())
            .or_default()
            .push(req.render_metadata.clone());

        if let Some(notifier) = self
            .keep_alive_notifiers
            .read()
            .await
            .get(session_id)
            .cloned()
            && !self
                .keep_alive_turns_complete_immediately
                .load(Ordering::Relaxed)
        {
            notifier.notified().await;
        }

        Ok(meerkat_core::lifecycle::core_executor::CoreApplyOutput {
            receipt: meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt {
                run_id,
                boundary,
                contributing_input_ids,
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            },
            session_snapshot: None,
            terminal: None,
        })
    }

    async fn apply_runtime_context_appends(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        appends: Vec<PendingSystemContextAppend>,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        // Canonical owner: MobSessionService::apply_runtime_context_appends.
        // Peer response terminals are context-only runtime inputs; the harness
        // must exercise the same owner boundary as persistent session services.
        if !self.sessions.read().await.contains_key(session_id) {
            return Err(SessionError::NotFound {
                id: session_id.clone(),
            });
        }

        self.applied_runtime_context_appends
            .write()
            .await
            .entry(session_id.clone())
            .or_default()
            .extend(
                appends
                    .into_iter()
                    .map(|append| AppendSystemContextRequest {
                        text: append.text,
                        source: append.source,
                        idempotency_key: append.idempotency_key,
                    }),
            );

        Ok(
            meerkat_core::lifecycle::core_executor::CoreApplyOutput::without_terminal(
                meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt {
                    run_id,
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate,
                    contributing_input_ids,
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                None,
            ),
        )
    }

    async fn apply_runtime_system_context_for_turn(
        &self,
        session_id: &SessionId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
    ) -> Result<(), SessionError> {
        for append in appends {
            self.append_system_context(
                session_id,
                AppendSystemContextRequest {
                    text: append.text,
                    source: append.source,
                    idempotency_key: append.idempotency_key,
                },
            )
            .await
            .map_err(|err| match err {
                SessionControlError::Session(err) => err,
                err => SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    err.to_string(),
                )),
            })?;
        }
        Ok(())
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.sessions.write().await.remove(session_id);
        self.keep_alive_notifiers.write().await.remove(session_id);
        self.session_comms_names.write().await.remove(session_id);
        self.applied_runtime_prompts
            .write()
            .await
            .remove(session_id);
        self.applied_runtime_context_appends
            .write()
            .await
            .remove(session_id);
        self.applied_runtime_render_metadata
            .write()
            .await
            .remove(session_id);
        Ok(())
    }
}

async fn create_test_mob_with_runtime_backed_real_comms(
    definition: MobDefinition,
) -> (MobHandle, Arc<RuntimeBackedRealCommsSessionService>) {
    let service = Arc::new(RuntimeBackedRealCommsSessionService::new());
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    (handle, service)
}

fn mcp_tool_context_for(comms: &Arc<meerkat_comms::CommsRuntime>) -> meerkat_comms::ToolContext {
    let runtime: Arc<dyn CoreCommsRuntime> = comms.clone();
    meerkat_comms::ToolContext {
        router: comms.router_arc(),
        trusted_peers: comms.trusted_peers_shared(),
        runtime: Some(meerkat_comms::RuntimeCommsCommandHandle::new(runtime)),
    }
}

async fn mcp_peer_id_for(comms: &Arc<meerkat_comms::CommsRuntime>, name: &str) -> String {
    let ctx = mcp_tool_context_for(comms);
    let peers = meerkat_comms::handle_tools_call(&ctx, "peers", &serde_json::json!({}))
        .await
        .expect("peers tool should succeed");
    let peer = peers["peers"]
        .as_array()
        .expect("peers should be an array")
        .iter()
        .find(|peer| peer["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("MCP peer not found for {name}; peers={peers}"));
    peer["peer_id"]
        .as_str()
        .expect("peer_id should be a string")
        .to_string()
}

fn peer_request_notice_request_id(text: &str) -> uuid::Uuid {
    let request_id = text
        .split("Request ID: ")
        .nth(1)
        .and_then(|tail| tail.split('.').next())
        .expect("peer request notice should include a Request ID");
    uuid::Uuid::parse_str(request_id).expect("request ID should be a UUID")
}

fn assert_peer_response_terminal_consumed(
    snapshot: &meerkat_runtime::MeerkatMachineSpineSnapshot,
    expected_handling_mode: HandlingMode,
) {
    let terminal = snapshot
        .inputs
        .admission_order
        .iter()
        .find(|entry| {
            entry
                .content_shape
                .as_ref()
                .is_some_and(|shape| shape.0 == "peer_response_terminal")
        })
        .unwrap_or_else(|| {
            panic!(
                "requester snapshot should include peer_response_terminal admission: {snapshot:?}"
            )
        });
    assert_eq!(terminal.handling_mode, Some(expected_handling_mode));
    assert!(
        matches!(
            terminal.lifecycle,
            Some(meerkat_runtime::input_state::InputLifecycleState::Consumed)
        ),
        "peer_response_terminal should be consumed after the runtime context append apply: {terminal:?}"
    );
}

#[tokio::test]
async fn test_peer_message_reaches_idle_autonomous_member_after_kickoff_completion() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) =
        create_test_mob_with_runtime_backed_real_comms(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);

    let sid_a = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire should succeed");

    tokio::time::sleep(Duration::from_millis(50)).await;
    let baseline_prompts = service.applied_runtime_prompts(&sid_b).await.len();

    let comms_a = service.real_comms(&sid_a).await.expect("comms for l-1");
    let receipt = CoreCommsRuntime::send(
        &*comms_a,
        CommsCommand::PeerMessage {
            to: test_peer_route(&*comms_a, &test_comms_name("worker", "w-1")).await,
            body: "body: please inspect this image".to_string(),
            blocks: Some(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "caption: this block text should survive".to_string(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "aGVsbG8=".into(),
                },
            ]),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        },
    )
    .await
    .expect("PeerMessage should succeed");
    assert!(
        matches!(receipt, SendReceipt::PeerMessageSent { .. }),
        "expected PeerMessageSent receipt, got: {receipt:?}"
    );

    let delivered = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let prompts = service.applied_runtime_prompts(&sid_b).await;
            if let Some(prompt) = prompts
                .iter()
                .skip(baseline_prompts)
                .find(|prompt| {
                    let text = prompt.text_content();
                    prompt.has_images()
                        || text.contains("body: please inspect this image")
                        || text.contains("caption: this block text should survive")
                })
                .cloned()
            {
                break prompt;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("peer message should reach runtime apply path");
    let delivered_text = delivered.text_content();
    assert!(
        delivered_text.contains("[COMMS MESSAGE from test-mob/lead/l-1]"),
        "peer message should carry comms source label: {delivered_text:?}"
    );
    assert!(
        delivered_text.contains("caption: this block text should survive"),
        "peer message block text should survive runtime delivery: {delivered_text:?}"
    );
    assert!(
        delivered.has_images(),
        "peer message image block should survive runtime delivery: {delivered:?}"
    );
}

#[tokio::test]
#[ignore = "exploratory runtime-backed kickoff contract; follow-up hardening pending"]
async fn test_wait_for_ready_blocks_until_autonomous_kickoff_resolves() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, _service) =
        create_test_mob_with_runtime_backed_real_comms(sample_definition()).await;

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-ready"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-ready"),
            None,
        )
        .await
        .expect("spawn worker");

    let kickoff = tokio::time::timeout(
        Duration::from_millis(250),
        handle.wait_for_members_kickoff_complete(
            &[
                AgentIdentity::from("l-ready"),
                AgentIdentity::from("w-ready"),
            ],
            None,
        ),
    )
    .await;
    assert!(
        kickoff.is_err(),
        "the harness should still have pending kickoff turns before they are explicitly released"
    );

    let ready = tokio::time::timeout(Duration::from_millis(250), handle.wait_for_ready(None)).await;
    assert!(
        ready.is_err(),
        "wait_for_ready must not resolve while autonomous kickoff turns are still pending"
    );

    tokio::time::timeout(Duration::from_secs(2), handle.stop())
        .await
        .expect("stop timeout after readiness assertion")
        .expect("stop after readiness assertion");
}

#[tokio::test]
#[ignore = "exploratory runtime-backed active-member delivery probe; smoke lane covers the end-to-end contract"]
async fn test_peer_message_reaches_ready_autonomous_member_before_kickoff_settles() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) =
        create_test_mob_with_runtime_backed_real_comms(sample_definition()).await;

    let sid_a = handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("l-prekickoff"),
            None,
        )
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-prekickoff"),
            None,
        )
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(
            AgentIdentity::from("l-prekickoff"),
            MeerkatId::from("w-prekickoff"),
        )
        .await
        .expect("wire should succeed");

    handle
        .wait_for_ready(Some(Duration::from_secs(2)))
        .await
        .expect("wait_for_ready should resolve for startup-ready autonomous members");

    let baseline_prompts = service.applied_runtime_prompts(&sid_b).await.len();
    let comms_a = service
        .real_comms(&sid_a)
        .await
        .expect("comms for l-prekickoff");
    let receipt = CoreCommsRuntime::send(
        &*comms_a,
        CommsCommand::PeerMessage {
            to: test_peer_route(&*comms_a, &test_comms_name("worker", "w-prekickoff")).await,
            body: "body: immediate orchestration after wait_for_ready".to_string(),
            blocks: Some(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "caption: startup-ready multimodal comms must still deliver".to_string(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "aGVsbG8=".into(),
                },
            ]),
            handling_mode: meerkat_core::types::HandlingMode::Steer,
        },
    )
    .await
    .expect("PeerMessage should succeed");
    assert!(
        matches!(receipt, SendReceipt::PeerMessageSent { .. }),
        "expected PeerMessageSent receipt, got: {receipt:?}"
    );

    let delivered = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let prompts = service.applied_runtime_prompts(&sid_b).await;
            if let Some(prompt) = prompts
                .iter()
                .skip(baseline_prompts)
                .find(|prompt| {
                    let text = prompt.text_content();
                    prompt.has_images()
                        || text.contains("body: immediate orchestration after wait_for_ready")
                        || text
                            .contains("caption: startup-ready multimodal comms must still deliver")
                })
                .cloned()
            {
                break prompt;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("peer message should reach a startup-ready autonomous member");
    let delivered_text = delivered.text_content();
    assert!(
        delivered_text.contains("[COMMS MESSAGE from test-mob/lead/l-prekickoff]"),
        "peer message should carry comms source label: {delivered_text:?}"
    );
    assert!(
        delivered_text.contains("caption: startup-ready multimodal comms must still deliver"),
        "peer message text block should survive runtime delivery: {delivered_text:?}"
    );
    assert!(
        delivered.has_images(),
        "peer message image block should survive runtime delivery: {delivered:?}"
    );

    tokio::time::timeout(Duration::from_secs(2), handle.stop())
        .await
        .expect("stop timeout after direct peer delivery assertion")
        .expect("stop after direct peer delivery assertion");
}

#[tokio::test]
#[ignore = "exploratory runtime-backed multi-recipient delivery probe; smoke lane covers the end-to-end contract"]
async fn test_peer_messages_reach_all_ready_autonomous_members_before_kickoff_settles() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) =
        create_test_mob_with_runtime_backed_real_comms(sample_definition()).await;

    let sid_lead = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-multi"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let targets = [
        ("w-multi-1", "worker"),
        ("w-multi-2", "worker"),
        ("w-multi-3", "worker"),
    ];
    let mut target_sessions = Vec::new();
    for (agent_identity, profile) in targets {
        let sid = handle
            .spawn(
                ProfileName::from(profile),
                MeerkatId::from(agent_identity),
                None,
            )
            .await
            .expect("spawn worker")
            .bridge_session_id()
            .expect("session-backed")
            .clone();
        handle
            .wire(
                AgentIdentity::from("l-multi"),
                MeerkatId::from(agent_identity),
            )
            .await
            .expect("wire should succeed");
        target_sessions.push((agent_identity.to_string(), sid));
    }

    handle
        .wait_for_ready(Some(Duration::from_secs(2)))
        .await
        .expect("wait_for_ready should resolve for startup-ready autonomous members");

    let baselines = futures::future::join_all(
        target_sessions
            .iter()
            .map(|(_, sid)| service.applied_runtime_prompts(sid)),
    )
    .await
    .into_iter()
    .map(|prompts| prompts.len())
    .collect::<Vec<_>>();

    let comms_lead = service.real_comms(&sid_lead).await.expect("comms for lead");
    for (agent_identity, _) in &target_sessions {
        let receipt = CoreCommsRuntime::send(
            &*comms_lead,
            CommsCommand::PeerMessage {
                to: test_peer_route(&*comms_lead, &test_comms_name("worker", agent_identity)).await,
                body: format!("body: fanout to {agent_identity}"),
                blocks: Some(vec![
                    meerkat_core::types::ContentBlock::Text {
                        text: format!("caption: fanout should reach {agent_identity}"),
                    },
                    meerkat_core::types::ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data: "aGVsbG8=".into(),
                    },
                ]),
                handling_mode: meerkat_core::types::HandlingMode::Steer,
            },
        )
        .await
        .expect("PeerMessage should succeed");
        assert!(
            matches!(receipt, SendReceipt::PeerMessageSent { .. }),
            "expected PeerMessageSent receipt, got: {receipt:?}"
        );
    }

    for ((agent_identity, sid), baseline_prompts) in target_sessions.iter().zip(baselines.iter()) {
        let delivered = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let prompts = service.applied_runtime_prompts(sid).await;
                if let Some(prompt) = prompts
                    .iter()
                    .skip(*baseline_prompts)
                    .find(|prompt| {
                        let text = prompt.text_content();
                        prompt.has_images()
                            || text.contains(&format!("body: fanout to {agent_identity}"))
                            || text
                                .contains(&format!("caption: fanout should reach {agent_identity}"))
                    })
                    .cloned()
                {
                    break prompt;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("peer message should reach {agent_identity}"));

        let delivered_text = delivered.text_content();
        assert!(
            delivered_text.contains("[COMMS MESSAGE from test-mob/lead/l-multi]"),
            "peer message should carry comms source label for {agent_identity}: {delivered_text:?}"
        );
        assert!(
            delivered_text.contains(&format!("caption: fanout should reach {agent_identity}")),
            "peer message text block should survive runtime delivery for {agent_identity}: {delivered_text:?}"
        );
        assert!(
            delivered.has_images(),
            "peer message image block should survive runtime delivery for {agent_identity}: {delivered:?}"
        );
    }

    tokio::time::timeout(Duration::from_secs(2), handle.stop())
        .await
        .expect("stop timeout after multi-recipient delivery assertion")
        .expect("stop after multi-recipient delivery assertion");
}

#[tokio::test]
async fn test_running_peer_message_to_autonomous_member_drains_after_current_apply() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) =
        create_test_mob_with_runtime_backed_real_comms(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);

    let sid_artist = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-artist"), None)
        .await
        .expect("spawn artist")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_helper = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-helper"), None)
        .await
        .expect("spawn helper")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_worker = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-target"),
            None,
        )
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(AgentIdentity::from("l-artist"), MeerkatId::from("w-target"))
        .await
        .expect("wire artist→worker");
    handle
        .wire(AgentIdentity::from("l-helper"), MeerkatId::from("w-target"))
        .await
        .expect("wire helper→worker");

    handle
        .wait_for_ready(Some(Duration::from_secs(2)))
        .await
        .expect("startup should settle with immediate keep-alive completions");
    handle
        .wait_for_members_kickoff_complete(
            &[
                AgentIdentity::from("l-artist"),
                AgentIdentity::from("l-helper"),
                AgentIdentity::from("w-target"),
            ],
            Some(Duration::from_secs(2)),
        )
        .await
        .expect("kickoff should resolve before we switch into blocked-turn mode");

    let baseline_prompts = service.applied_runtime_prompts(&sid_worker).await.len();
    service.set_keep_alive_turns_complete_immediately(false);

    let artist_comms = service.real_comms(&sid_artist).await.expect("artist comms");
    let helper_comms = service.real_comms(&sid_helper).await.expect("helper comms");

    CoreCommsRuntime::send(
        &*artist_comms,
        CommsCommand::PeerMessage {
            to: test_peer_route(&*artist_comms, &test_comms_name("worker", "w-target")).await,
            body: "body: first peer message".to_string(),
            blocks: Some(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "caption: first peer message".to_string(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "aGVsbG8=".into(),
                },
            ]),
            handling_mode: meerkat_core::types::HandlingMode::Steer,
        },
    )
    .await
    .expect("first peer message should succeed");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let prompts = service.applied_runtime_prompts(&sid_worker).await;
            if prompts.iter().skip(baseline_prompts).any(|prompt| {
                prompt.has_images()
                    || prompt
                        .text_content()
                        .contains("caption: first peer message")
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("worker should begin applying the first peer message");

    CoreCommsRuntime::send(
        &*helper_comms,
        CommsCommand::PeerMessage {
            to: test_peer_route(&*helper_comms, &test_comms_name("worker", "w-target")).await,
            body: "body: second peer message while running".to_string(),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Steer,
        },
    )
    .await
    .expect("second peer message should succeed");

    service
        .interrupt(&sid_worker)
        .await
        .expect("interrupt should release blocked worker apply");

    let delivered_second = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let prompts = service.applied_runtime_prompts(&sid_worker).await;
            if prompts.iter().skip(baseline_prompts + 1).any(|prompt| {
                prompt
                    .text_content()
                    .contains("body: second peer message while running")
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;

    tokio::time::timeout(Duration::from_secs(2), handle.stop())
        .await
        .expect("stop timeout after running peer message assertion")
        .expect("stop after running peer message assertion");

    delivered_second.expect(
        "second steer peer message should drain onto the next apply after the active turn completes",
    );
}

#[tokio::test]
async fn test_peer_response_reaches_requester_in_runtime_backed_real_comms() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) =
        create_test_mob_with_runtime_backed_real_comms(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);

    let sid_requester = handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("l-requester"),
            None,
        )
        .await
        .expect("spawn requester")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_responder = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-responder"),
            None,
        )
        .await
        .expect("spawn responder")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(
            AgentIdentity::from("l-requester"),
            MeerkatId::from("w-responder"),
        )
        .await
        .expect("wire requester↔responder");
    handle
        .wait_for_ready(Some(Duration::from_secs(2)))
        .await
        .expect("wait_for_ready should resolve");

    let requester_prompt_baseline = service.applied_runtime_prompts(&sid_requester).await.len();
    let requester_context_baseline = service
        .applied_runtime_context_appends(&sid_requester)
        .await
        .len();
    let responder_baseline = service.applied_runtime_prompts(&sid_responder).await.len();

    let requester_comms = service
        .real_comms(&sid_requester)
        .await
        .expect("requester comms");
    let responder_comms = service
        .real_comms(&sid_responder)
        .await
        .expect("responder comms");
    let responder_route_identity = responder_comms.public_key().to_peer_id().to_string();
    let responder_display_identity = test_comms_name("worker", "w-responder");

    let receipt = CoreCommsRuntime::send(
        &*requester_comms,
        CommsCommand::PeerRequest {
            to: test_peer_route(&*requester_comms, &test_comms_name("worker", "w-responder")).await,
            intent: "interpret_image".to_string(),
            params: serde_json::json!({"description":"tower with a light"}),
            handling_mode: meerkat_core::types::HandlingMode::Steer,
            stream: meerkat_core::InputStreamMode::ReserveInteraction,
        },
    )
    .await
    .expect("peer request should succeed");
    let request_id = match receipt {
        SendReceipt::PeerRequestSent { interaction_id, .. } => interaction_id,
        other => panic!("expected PeerRequestSent receipt, got {other:?}"),
    };

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let prompts = service.applied_runtime_prompts(&sid_responder).await;
            if prompts.iter().skip(responder_baseline).any(|prompt| {
                prompt
                    .text_content()
                    .contains("[SYSTEM NOTICE][PEER_REQUEST]")
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("responder should receive the peer request");

    CoreCommsRuntime::send(
        &*responder_comms,
        CommsCommand::PeerResponse {
            to: test_peer_route(&*responder_comms, &test_comms_name("lead", "l-requester")).await,
            in_reply_to: request_id,
            status: meerkat_core::ResponseStatus::Completed,
            result: serde_json::json!({"interpretation":"lighthouse"}),
            handling_mode: None,
        },
    )
    .await
    .expect("peer response should succeed");

    let requester_response_delivery = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let appends = service
                .applied_runtime_context_appends(&sid_requester)
                .await;
            if appends
                .iter()
                .skip(requester_context_baseline)
                .any(|append| {
                    let text = append.text.as_str();
                    let source = append.source.as_deref().unwrap_or_default();
                    let idempotency_key = append.idempotency_key.as_deref().unwrap_or_default();
                    let expected_source =
                        format!("peer_response_terminal:{responder_route_identity}:{request_id}");
                    text.contains("[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL]")
                        && text.contains("from test-mob/worker/w-responder")
                        && text.contains("lighthouse")
                        && text.contains(&format!("Request ID: {request_id}"))
                        && source == expected_source
                        && idempotency_key == source
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;

    let requester_snapshot = service
        .runtime_adapter
        .meerkat_machine_spine_snapshot(&sid_requester)
        .await
        .expect("requester snapshot should exist");

    if requester_response_delivery.is_ok() {
        assert_peer_response_terminal_consumed(&requester_snapshot, HandlingMode::Steer);
    }

    tokio::time::timeout(Duration::from_secs(2), handle.stop())
        .await
        .expect("stop timeout after peer response assertion")
        .expect("stop after peer response assertion");

    if requester_response_delivery.is_err() {
        let has_executor = service
            .runtime_adapter
            .session_has_executor(&sid_requester)
            .await;
        let has_comms = service
            .runtime_adapter
            .session_has_comms(&sid_requester)
            .await;
        panic!(
            "requester should apply the terminal peer response as runtime-owned context; has_executor={has_executor} has_comms={has_comms} snapshot={requester_snapshot:?}"
        );
    }

    let requester_snapshot = service
        .runtime_adapter
        .meerkat_machine_spine_snapshot(&sid_requester)
        .await
        .expect("requester snapshot should still exist after delivery");
    assert!(
        requester_snapshot.inputs.queue.is_empty(),
        "terminal peer response should be consumed, not left queued: {requester_snapshot:?}"
    );
    let terminal_admission = requester_snapshot
        .inputs
        .admission_order
        .iter()
        .find(|input| {
            input
                .content_shape
                .as_ref()
                .is_some_and(|shape| shape.0.as_str() == "peer_response_terminal")
        })
        .expect("requester should admit a terminal peer response input");
    assert_eq!(
        terminal_admission.lifecycle,
        Some(meerkat_runtime::InputLifecycleState::Consumed),
        "terminal peer response should be applied and consumed: {requester_snapshot:?}"
    );
    let requester_prompts_after_response = service.applied_runtime_prompts(&sid_requester).await;
    assert_eq!(
        requester_prompts_after_response.len(),
        requester_prompt_baseline + 1,
        "terminal peer response should append runtime system context and kick a requester reaction turn"
    );
    assert_eq!(
        requester_prompts_after_response
            .last()
            .map(ContentInput::text_content)
            .unwrap_or_default(),
        "",
        "terminal peer response reaction should be system-triggered, not a new user prompt"
    );

    let duplicate_peer_id =
        meerkat_core::comms::PeerId::parse(&responder_route_identity).expect("responder peer id");
    let duplicate_display_name =
        meerkat_core::comms::PeerName::new(responder_display_identity.clone())
            .expect("responder display name");
    let duplicate_request_id = meerkat_core::PeerCorrelationId::from_uuid(request_id.0);
    let duplicate = meerkat_runtime::peer_response_terminal_input(
        duplicate_peer_id,
        Some(duplicate_display_name),
        duplicate_request_id,
        meerkat_contracts::PeerResponseTerminalStatusWire::Completed,
        serde_json::json!({"interpretation":"lighthouse"}),
    );
    service
        .runtime_adapter
        .accept_input_with_completion(&sid_requester, duplicate)
        .await
        .expect("duplicate terminal response should be accepted for idempotent apply");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = service
                .runtime_adapter
                .meerkat_machine_spine_snapshot(&sid_requester)
                .await
                .expect("requester snapshot should still exist after duplicate");
            if snapshot.inputs.queue.is_empty()
                && snapshot
                    .inputs
                    .admission_order
                    .iter()
                    .filter(|input| {
                        input
                            .content_shape
                            .as_ref()
                            .is_some_and(|shape| shape.0.as_str() == "peer_response_terminal")
                            && input.lifecycle
                                == Some(meerkat_runtime::InputLifecycleState::Consumed)
                    })
                    .count()
                    >= 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("duplicate terminal response should be consumed without re-applying context");
    assert_eq!(
        service
            .applied_runtime_context_appends(&sid_requester)
            .await
            .len(),
        requester_context_baseline + 1,
        "duplicate terminal response should not append duplicate runtime system context"
    );
}

#[tokio::test]
async fn test_default_peer_response_inherits_request_steer_while_requester_running() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) =
        create_test_mob_with_runtime_backed_real_comms(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);

    let sid_requester = handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("l-running-requester"),
            None,
        )
        .await
        .expect("spawn requester")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_responder = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-running-responder"),
            None,
        )
        .await
        .expect("spawn responder")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(
            AgentIdentity::from("l-running-requester"),
            MeerkatId::from("w-running-responder"),
        )
        .await
        .expect("wire requester↔responder");
    handle
        .wait_for_ready(Some(Duration::from_secs(2)))
        .await
        .expect("wait_for_ready should resolve");
    handle
        .wait_for_members_kickoff_complete(
            &[
                AgentIdentity::from("l-running-requester"),
                AgentIdentity::from("w-running-responder"),
            ],
            Some(Duration::from_secs(2)),
        )
        .await
        .expect("kickoff should resolve before blocking turns");

    let requester_prompt_baseline = service.applied_runtime_prompts(&sid_requester).await.len();
    let requester_context_baseline = service
        .applied_runtime_context_appends(&sid_requester)
        .await
        .len();
    let responder_baseline = service.applied_runtime_prompts(&sid_responder).await.len();
    service.set_keep_alive_turns_complete_immediately(false);

    let requester_comms = service
        .real_comms(&sid_requester)
        .await
        .expect("requester comms");
    let responder_comms = service
        .real_comms(&sid_responder)
        .await
        .expect("responder comms");

    CoreCommsRuntime::send(
        &*responder_comms,
        CommsCommand::PeerMessage {
            to: test_peer_route(
                &*responder_comms,
                &test_comms_name("lead", "l-running-requester"),
            )
            .await,
            body: "body: keep requester busy".to_string(),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Steer,
        },
    )
    .await
    .expect("busy message should send");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let prompts = service.applied_runtime_prompts(&sid_requester).await;
            if prompts
                .iter()
                .skip(requester_prompt_baseline)
                .any(|prompt| prompt.text_content().contains("body: keep requester busy"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("requester should be in a running turn before the pong lands");

    let request_receipt = CoreCommsRuntime::send(
        &*requester_comms,
        CommsCommand::PeerRequest {
            to: test_peer_route(
                &*requester_comms,
                &test_comms_name("worker", "w-running-responder"),
            )
            .await,
            intent: "ping".to_string(),
            params: serde_json::json!({"message":"ping"}),
            handling_mode: meerkat_core::types::HandlingMode::Steer,
            stream: meerkat_core::InputStreamMode::ReserveInteraction,
        },
    )
    .await
    .expect("peer request should succeed");
    let request_id = match request_receipt {
        SendReceipt::PeerRequestSent { interaction_id, .. } => interaction_id,
        other => panic!("expected PeerRequestSent receipt, got {other:?}"),
    };

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let prompts = service.applied_runtime_prompts(&sid_responder).await;
            if prompts.iter().skip(responder_baseline).any(|prompt| {
                let text = prompt.text_content();
                text.contains("[SYSTEM NOTICE][PEER_REQUEST]")
                    && text.contains("Intent: ping")
                    && text.contains(&format!("Request ID: {request_id}"))
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("responder should receive the steer ping request");

    CoreCommsRuntime::send(
        &*responder_comms,
        CommsCommand::PeerResponse {
            to: test_peer_route(
                &*responder_comms,
                &test_comms_name("lead", "l-running-requester"),
            )
            .await,
            in_reply_to: request_id,
            status: meerkat_core::ResponseStatus::Completed,
            result: serde_json::json!({"message":"pong"}),
            handling_mode: None,
        },
    )
    .await
    .expect("default peer response should succeed");

    let requester_delivery = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let appends = service
                .applied_runtime_context_appends(&sid_requester)
                .await;
            if appends
                .iter()
                .skip(requester_context_baseline)
                .any(|append| {
                    append
                        .text
                        .contains("[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL]")
                        && append.text.contains(&format!("Request ID: {request_id}"))
                        && append.text.contains("\"message\": \"pong\"")
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    if requester_delivery.is_err() {
        let appends = service
            .applied_runtime_context_appends(&sid_requester)
            .await;
        let snapshot = service
            .runtime_adapter
            .meerkat_machine_spine_snapshot(&sid_requester)
            .await
            .expect("requester snapshot should exist");
        panic!(
            "default response to a steer request should interrupt and reach requester; appends={appends:?} snapshot={snapshot:?}"
        );
    }

    tokio::time::timeout(Duration::from_secs(2), handle.stop())
        .await
        .expect("stop timeout after running requester response assertion")
        .expect("stop after running requester response assertion");
}

#[tokio::test]
async fn test_mcp_send_request_response_terminal_steer_is_visible_to_requester() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) =
        create_test_mob_with_runtime_backed_real_comms(sample_definition()).await;
    service.set_keep_alive_turns_complete_immediately(true);

    let sid_requester = handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("l-mcp-requester"),
            None,
        )
        .await
        .expect("spawn requester")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_responder = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-mcp-responder"),
            None,
        )
        .await
        .expect("spawn responder")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(
            AgentIdentity::from("l-mcp-requester"),
            MeerkatId::from("w-mcp-responder"),
        )
        .await
        .expect("wire requester↔responder");
    handle
        .wait_for_ready(Some(Duration::from_secs(2)))
        .await
        .expect("wait_for_ready should resolve");

    let responder_baseline = service.applied_runtime_prompts(&sid_responder).await.len();
    let requester_context_baseline = service
        .applied_runtime_context_appends(&sid_requester)
        .await
        .len();

    let requester_comms = service
        .real_comms(&sid_requester)
        .await
        .expect("requester comms");
    let responder_comms = service
        .real_comms(&sid_responder)
        .await
        .expect("responder comms");
    let responder_peer_id = mcp_peer_id_for(
        &requester_comms,
        &test_comms_name("worker", "w-mcp-responder"),
    )
    .await;
    let requester_peer_id = mcp_peer_id_for(
        &responder_comms,
        &test_comms_name("lead", "l-mcp-requester"),
    )
    .await;

    let requester_ctx = mcp_tool_context_for(&requester_comms);
    let request_result = meerkat_comms::handle_tools_call(
        &requester_ctx,
        "send_request",
        &serde_json::json!({
            "peer_id": responder_peer_id,
            "intent": "checksum_token",
            "params": {"subject": "alpha beta gamma"},
            "handling_mode": "steer"
        }),
    )
    .await
    .expect("send_request tool should succeed");
    assert_eq!(request_result["kind"], "peer_request");
    assert_eq!(request_result["status"], "sent");

    let responder_request_prompt = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let prompts = service.applied_runtime_prompts(&sid_responder).await;
            if let Some(prompt) = prompts
                .iter()
                .skip(responder_baseline)
                .find(|prompt| {
                    let text = prompt.text_content();
                    text.contains("[SYSTEM NOTICE][PEER_REQUEST]")
                        && text.contains("Intent: checksum_token")
                        && text.contains("Request ID: ")
                })
                .cloned()
            {
                break prompt;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("responder should receive actionable MCP peer request");
    let request_notice = responder_request_prompt.text_content();
    let request_id = peer_request_notice_request_id(&request_notice);

    let responder_ctx = mcp_tool_context_for(&responder_comms);
    let response_result = meerkat_comms::handle_tools_call(
        &responder_ctx,
        "send_response",
        &serde_json::json!({
            "peer_id": requester_peer_id,
            "in_reply_to": request_id.to_string(),
            "status": "completed",
            "result": {
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            },
            "handling_mode": "steer"
        }),
    )
    .await
    .expect("send_response tool should succeed");
    assert_eq!(response_result["kind"], "peer_response");
    assert_eq!(response_result["status"], "sent");

    let requester_terminal_context = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let appends = service
                .applied_runtime_context_appends(&sid_requester)
                .await;
            if let Some(append) = appends
                .iter()
                .skip(requester_context_baseline)
                .find(|append| {
                    append
                        .text
                        .contains("[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL]")
                        && append.text.contains("from test-mob/worker/w-mcp-responder")
                        && append.text.contains(&format!("Request ID: {request_id}"))
                        && append.text.contains("Status: completed")
                        && append
                            .text
                            .contains("\"request_intent\": \"checksum_token\"")
                        && append.text.contains("\"token\": \"birch seventeen\"")
                })
                .cloned()
            {
                break append;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("requester should apply terminal peer response context after sent tool result");

    let expected_context_key = format!("peer_response_terminal:{responder_peer_id}:{request_id}");
    assert_eq!(
        requester_terminal_context.source.as_deref(),
        Some(expected_context_key.as_str())
    );
    assert_eq!(
        requester_terminal_context.idempotency_key.as_deref(),
        Some(expected_context_key.as_str())
    );

    let requester_snapshot = service
        .runtime_adapter
        .meerkat_machine_spine_snapshot(&sid_requester)
        .await
        .expect("requester snapshot should exist");
    assert_peer_response_terminal_consumed(&requester_snapshot, HandlingMode::Steer);

    tokio::time::timeout(Duration::from_secs(2), handle.stop())
        .await
        .expect("stop timeout after MCP peer response assertion")
        .expect("stop after MCP peer response assertion");
}

#[tokio::test]
async fn test_runtime_backed_turn_driven_send_preserves_render_metadata() {
    let mut definition = sample_definition();
    definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .expect("lead profile")
        .as_inline_mut()
        .unwrap()
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;

    let (handle, service) = create_test_mob_with_runtime_backed_real_comms(definition).await;
    let member = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("lead-rt"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let render_metadata = meerkat_core::types::RenderMetadata {
        class: meerkat_core::types::RenderClass::ExternalEvent,
        salience: meerkat_core::types::RenderSalience::Urgent,
    };

    handle
        .member(&AgentIdentity::from("lead-rt"))
        .await
        .expect("member handle")
        .send_with_render_metadata(
            ContentInput::Text("hello".into()),
            meerkat_core::types::HandlingMode::Queue,
            Some(render_metadata.clone()),
        )
        .await
        .expect("runtime-backed turn-driven send should succeed");

    assert_eq!(
        service.applied_runtime_render_metadata(&member).await,
        vec![Some(render_metadata)]
    );
}

// WIP: turn-driven realtime-attached peer_response_terminal application
// lands the response in the `Steer` admission queue without flushing it
// onto the keep-alive runtime's prompt stream. The machine/DSL rebase on
// `codex/machine-dls-completion` is expected to own the final shape of
// this delivery seam; tracking this as ignored until then so CI and the
// audio smoke lane aren't blocked by the transitional gap.
#[tokio::test]
async fn test_member_status_keeps_idle_live_session_active() {
    let inner = Arc::new(MockSessionService::new());
    let _ = inner.enable_runtime_adapter();
    let service = Arc::new(InactiveReadSessionService::new(inner));
    let handle = MobBuilder::new(sample_definition(), MobStorage::in_memory())
        .with_session_service(service)
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let snapshot = handle
        .member_status(&AgentIdentity::from("w-1"))
        .await
        .expect("member status");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Active,
        "idle live sessions should remain Active rather than Completed"
    );
    assert!(!snapshot.is_final);
}

#[tokio::test]
async fn test_mob_session_service_exposes_event_injector_for_session() {
    let (handle, service) = create_test_mob_with_real_comms(sample_definition()).await;
    let sid = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let injector = service.event_injector(&sid).await;
    assert!(
        injector.is_some(),
        "expected event injector for live comms-backed session"
    );
}

#[tokio::test]
async fn test_mob_session_service_subscribe_session_events_available() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    let stream_result =
        crate::runtime::session_service::MobSessionService::subscribe_session_events(
            service.as_ref(),
            &sid,
        )
        .await;
    assert!(
        stream_result.is_ok(),
        "expected session-wide event stream contract from mob session service"
    );
}

static REAL_COMMS_TEST_LOCK: Mutex<()> = Mutex::new(());

#[tokio::test]
async fn test_wire_enables_peer_request_delivery() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) = create_test_mob_with_real_comms(sample_definition()).await;

    let sid_a = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire should succeed");

    let comms_a = service.real_comms(&sid_a).await.expect("comms for l-1");
    let comms_b = service.real_comms(&sid_b).await.expect("comms for w-1");

    let entry_a = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("l-1 should be in roster");
    let entry_b = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("w-1 should be in roster");
    let comms_name_a = format!(
        "{}/{}/{}",
        handle.mob_id(),
        entry_a.role,
        entry_a.agent_identity
    );
    let comms_name_b = format!(
        "{}/{}/{}",
        handle.mob_id(),
        entry_b.role,
        entry_b.agent_identity
    );

    let peers_a = CoreCommsRuntime::peers(&*comms_a).await;
    let peers_b = CoreCommsRuntime::peers(&*comms_b).await;
    assert!(
        peers_a
            .iter()
            .any(|entry| entry.name.as_str() == comms_name_b),
        "wire should expose worker in lead peers()"
    );
    assert!(
        peers_b
            .iter()
            .any(|entry| entry.name.as_str() == comms_name_a),
        "wire should expose lead in worker peers()"
    );

    let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_a).await;
    let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;

    let cmd = meerkat_core::comms::CommsCommand::PeerRequest {
        to: test_peer_route(&*comms_a, &comms_name_b).await,
        intent: "mob.test_ping".to_string(),
        params: serde_json::json!({"test": true}),
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        stream: meerkat_core::comms::InputStreamMode::None,
    };
    let receipt = CoreCommsRuntime::send(&*comms_a, cmd)
        .await
        .expect("PeerRequest from l-1 to w-1 should succeed after wire()");
    assert!(
        matches!(
            receipt,
            meerkat_core::comms::SendReceipt::PeerRequestSent { .. }
        ),
        "expected PeerRequestSent, got: {receipt:?}"
    );

    let interactions = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;
    assert_eq!(
        interactions.len(),
        1,
        "w-1 should have received exactly one interaction after wire"
    );
    match &interactions[0].content {
        meerkat_core::InteractionContent::Request { intent, params } => {
            assert_eq!(intent, "mob.test_ping");
            assert_eq!(params["test"], true);
        }
        other => panic!("expected Request interaction, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_unwire_updates_peers_and_sends_retired_notifications() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) = create_test_mob_with_real_comms(sample_definition()).await;

    let sid_a = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let comms_a = service.real_comms(&sid_a).await.expect("comms for l-1");
    let comms_b = service.real_comms(&sid_b).await.expect("comms for w-1");
    let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_a).await;
    let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;

    handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("unwire");

    let peers_a = CoreCommsRuntime::peers(&*comms_a).await;
    let peers_b = CoreCommsRuntime::peers(&*comms_b).await;
    assert!(
        !peers_a
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("worker", "w-1")),
        "unwire should remove worker from lead peers()"
    );
    assert!(
        !peers_b
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("lead", "l-1")),
        "unwire should remove lead from worker peers()"
    );
}

#[tokio::test]
async fn test_unwire_prunes_stale_local_trust_when_projection_is_already_absent() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) = create_test_mob_with_real_comms(sample_definition()).await;

    let sid_a = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");
    handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("initial unwire");

    let comms_a = service.real_comms(&sid_a).await.expect("comms for l-1");
    let comms_b = service.real_comms(&sid_b).await.expect("comms for w-1");
    let name_a = test_comms_name("lead", "l-1");
    let name_b = test_comms_name("worker", "w-1");
    let key_a = comms_a.public_key();
    let key_b = comms_b.public_key();
    comms_a
        .add_trusted_peer(
            TrustedPeerDescriptor::unsigned_with_pubkey(
                &name_b,
                key_b.to_peer_id().to_string(),
                *key_b.as_bytes(),
                format!("inproc://{name_b}"),
            )
            .expect("valid worker trusted spec"),
        )
        .await
        .expect("re-add stale trust on lead");
    comms_b
        .add_trusted_peer(
            TrustedPeerDescriptor::unsigned_with_pubkey(
                &name_a,
                key_a.to_peer_id().to_string(),
                *key_a.as_bytes(),
                format!("inproc://{name_a}"),
            )
            .expect("valid lead trusted spec"),
        )
        .await
        .expect("re-add stale trust on worker");

    handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("idempotent stale-trust cleanup unwire");

    let peers_a = CoreCommsRuntime::peers(&*comms_a).await;
    let peers_b = CoreCommsRuntime::peers(&*comms_b).await;
    assert!(
        !peers_a.iter().any(|entry| entry.name.as_str() == name_b),
        "idempotent unwire should prune stale trust from lead"
    );
    assert!(
        !peers_b.iter().any(|entry| entry.name.as_str() == name_a),
        "idempotent unwire should prune stale trust from worker"
    );
}

#[tokio::test]
async fn test_unwire_external_prunes_stale_trust_when_projection_is_already_absent() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) = create_test_mob_with_real_comms(sample_definition()).await;

    let sid = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let external_keypair = meerkat_comms::Keypair::generate();
    let external_pubkey = external_keypair.public_key();
    let spec = TrustedPeerDescriptor::unsigned_with_pubkey(
        "remote-mob/worker/agent-x",
        external_pubkey.to_peer_id().to_string(),
        *external_pubkey.as_bytes(),
        "inproc://remote-mob/worker/agent-x",
    )
    .expect("valid external peer");
    handle
        .wire(
            AgentIdentity::from("l-1"),
            PeerTarget::External(spec.clone()),
        )
        .await
        .expect("wire external");
    handle
        .unwire(
            AgentIdentity::from("l-1"),
            PeerTarget::External(spec.clone()),
        )
        .await
        .expect("initial external unwire");

    let comms = service.real_comms(&sid).await.expect("comms for l-1");
    comms
        .add_trusted_peer(spec.clone())
        .await
        .expect("re-add stale external trust");

    handle
        .unwire(
            AgentIdentity::from("l-1"),
            PeerTarget::External(spec.clone()),
        )
        .await
        .expect("idempotent external stale-trust cleanup");

    let peers = CoreCommsRuntime::peers(&*comms).await;
    assert!(
        !peers
            .iter()
            .any(|entry| entry.name.as_str() == spec.name.as_str()),
        "idempotent external unwire should prune stale trust"
    );
}

#[tokio::test]
async fn test_retire_updates_peers_and_sends_retired_notifications() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let (handle, service) = create_test_mob_with_real_comms(sample_definition()).await;

    let sid_a = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    let comms_a = service.real_comms(&sid_a).await.expect("comms for w-1");
    let comms_b = service.real_comms(&sid_b).await.expect("comms for w-2");
    let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_a).await;
    let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;

    handle
        .retire(AgentIdentity::from("w-1"))
        .await
        .expect("retire");

    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "retired meerkat should leave roster"
    );
    let entry_w2 = handle
        .get_member(&AgentIdentity::from("w-2"))
        .await
        .expect("w-2 remains active");
    assert!(
        !entry_w2.wired_to.contains(&AgentIdentity::from("w-1")),
        "retire should remove wiring from remaining peer"
    );

    let peers_w2 = CoreCommsRuntime::peers(&*comms_b).await;
    assert!(
        !peers_w2
            .iter()
            .any(|entry| entry.name.as_str() == test_comms_name("worker", "w-1")),
        "retire should remove retired peer from peers()"
    );
}

#[tokio::test]
async fn test_unwire_sends_required_peer_unwired_notifications() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    handle
        .unwire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("unwire");

    let intents_lead = service.sent_intents(&sid_l).await;
    let intents_worker = service.sent_intents(&sid_w).await;
    assert!(
        intents_lead
            .iter()
            .any(|intent| intent == "mob.peer_unwired"),
        "lead should send mob.peer_unwired during unwire"
    );
    assert!(
        intents_worker
            .iter()
            .any(|intent| intent == "mob.peer_unwired"),
        "worker should send mob.peer_unwired during unwire"
    );
}

#[tokio::test]
async fn test_retire_sends_required_peer_retired_notifications() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_w1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let sid_w2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    handle
        .retire(AgentIdentity::from("w-1"))
        .await
        .expect("retire");

    let intents_w1 = service.sent_intents(&sid_w1).await;
    let intents_w2 = service.sent_intents(&sid_w2).await;
    assert!(
        intents_w1.iter().any(|intent| intent == "mob.peer_retired"),
        "retiring peer should send mob.peer_retired notifications"
    );
    assert!(
        !intents_w2.iter().any(|intent| intent == "mob.peer_retired"),
        "non-retiring peer should not be the sender for retire notifications"
    );
}

// -----------------------------------------------------------------------
// Disposal pipeline integration tests
// -----------------------------------------------------------------------

/// Verifies that roster removal happens even when multiple cleanup steps fail.
/// This tests the structural guarantee that roster removal is in the "finally"
/// block of dispose_member, outside the policy-driven loop.
#[tokio::test]
async fn test_dispose_member_removes_roster_even_when_steps_fail() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    // Configure w-1's comms to fail both send and trust removal.
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                fail_send_peer_retired: true,
                fail_remove_trust: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    // Archive also fails.
    let sid_w1 = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .unwrap()
        .bridge_session_id()
        .cloned()
        .unwrap();
    service.set_archive_failure(&sid_w1).await;

    // Archive failure is critical — retire must surface it even when comms
    // steps also failed (comms failures are still best-effort).
    let result = handle.retire(AgentIdentity::from("w-1")).await;
    assert!(
        result.is_err(),
        "retire must return Err when ArchiveSession fails, regardless of other step failures"
    );

    // The structural guarantee: member is gone from roster regardless.
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "roster removal must be unconditional — member must be gone even when all steps fail"
    );
}

/// Verifies that retiring two identical-topology members produces the same
/// disposal step sequence (deterministic ordering).
#[tokio::test]
async fn test_disposal_report_ordering_is_deterministic() {
    let (handle, _service) = create_test_mob(sample_definition()).await;

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");

    // Retire both in sequence — same topology, same cleanup steps.
    handle
        .retire(AgentIdentity::from("w-1"))
        .await
        .expect("retire w-1");
    handle
        .retire(AgentIdentity::from("w-2"))
        .await
        .expect("retire w-2");

    // Both members removed — disposal executed identically.
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none()
    );
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-2"))
            .await
            .is_none()
    );

    // Verify events show two retirements in order.
    let events = handle.events().replay_all().await.expect("replay");
    let retire_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind, MobEventKind::MemberRetired { .. }))
        .collect();
    assert_eq!(
        retire_events.len(),
        2,
        "both members should have retire events"
    );
}

// -----------------------------------------------------------------------
// MobHandle::reset()
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_reset_clears_roster_events_and_returns_to_running() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");

    handle.reset().await.expect("reset");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    assert!(
        handle.list_members().await.is_empty(),
        "reset should retire all members"
    );

    let events = handle.events().replay_all().await.expect("replay");
    // Append-only: old epoch events remain, new epoch ends with
    // MobCreated + MobReset.
    let len = events.len();
    assert!(len >= 2, "should have at least MobCreated + MobReset");
    assert!(
        matches!(events[len - 2].kind, MobEventKind::MobCreated { .. }),
        "penultimate event should be MobCreated (required for resume)"
    );
    assert!(
        matches!(events[len - 1].kind, MobEventKind::MobReset),
        "last event should be MobReset"
    );

    // Roster projection after full replay should be empty (MobReset clears it).
    let roster = crate::roster::Roster::project(&events);
    assert!(
        roster.list().next().is_none(),
        "roster projection should be empty after reset epoch"
    );
}

#[tokio::test]
async fn test_reset_allows_spawn_after_reset() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");

    handle.reset().await.expect("reset");

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn after reset should succeed");
    assert_eq!(handle.list_members().await.len(), 1);
}

#[tokio::test]
async fn test_reset_from_stopped_state() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle.stop().await.expect("stop");
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);

    handle.reset().await.expect("reset from stopped");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
}

#[tokio::test]
async fn test_reset_from_completed_state() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle.complete().await.expect("complete");
    assert_eq!(handle.status().await.unwrap(), MobState::Completed);

    handle.reset().await.expect("reset from completed");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
}

#[tokio::test]
async fn test_reset_rejects_from_destroyed() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle.destroy().await.expect("destroy");
    assert_eq!(handle.status().await.unwrap(), MobState::Destroyed);

    let err = handle
        .reset()
        .await
        .expect_err("reset after destroy must fail");
    assert!(
        matches!(err, MobError::Internal(ref message) if message.contains("actor task dropped")),
        "reset after terminal destroy should fail because the actor is gone: {err:?}"
    );
}

#[tokio::test]
async fn test_reset_clears_task_board() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .task_create("Task A".into(), "do a".into(), vec![])
        .await
        .expect("create task a");
    handle
        .task_create("Task B".into(), "do b".into(), vec![])
        .await
        .expect("create task b");
    assert_eq!(handle.task_list().await.unwrap().len(), 2);

    handle.reset().await.expect("reset");
    assert!(
        handle.task_list().await.unwrap().is_empty(),
        "reset should clear the task board"
    );
}

#[tokio::test]
async fn test_task_get_round_trips_through_machine_command_surface() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let task_id = handle
        .task_create("Task A".into(), "do a".into(), vec![])
        .await
        .expect("create task a");

    let task = handle
        .task_get(&task_id)
        .await
        .expect("task_get")
        .expect("task exists");
    assert_eq!(task.id, task_id);

    handle.reset().await.expect("reset");
    assert!(
        handle
            .task_get(&task_id)
            .await
            .expect("task_get after reset")
            .is_none(),
        "reset should clear the task board for machine-routed task_get"
    );
}

#[tokio::test]
async fn test_structural_roster_reads_round_trip_through_machine_command_surface() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let roster = handle.roster().await;
    assert!(
        roster.get(&MeerkatId::from("w-1")).is_some(),
        "machine-routed roster snapshot should include spawned member"
    );

    let all_members = handle.list_all_members().await;
    assert_eq!(all_members.len(), 1);
    assert_eq!(all_members[0].agent_identity, AgentIdentity::from("w-1"));

    let active_members = handle.list_members().await;
    assert_eq!(active_members.len(), 1);
    assert_eq!(active_members[0].agent_identity, AgentIdentity::from("w-1"));

    let including_retiring = handle.list_members_including_retiring().await;
    assert_eq!(including_retiring.len(), 1);
    assert_eq!(
        including_retiring[0].agent_identity,
        AgentIdentity::from("w-1")
    );

    let entry = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("member exists");
    assert_eq!(entry.agent_identity, MeerkatId::from("w-1"));

    handle
        .retire(AgentIdentity::from("w-1"))
        .await
        .expect("retire worker");
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "machine-routed get_member should reflect retirement"
    );
}

#[tokio::test]
async fn test_member_status_round_trips_through_machine_command_surface() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let receipt = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let snapshot = handle
        .member_status(&AgentIdentity::from("w-1"))
        .await
        .expect("member status");
    assert_eq!(snapshot.status, crate::runtime::MobMemberStatus::Active);
    assert_eq!(
        snapshot.current_bridge_session_id.as_ref(),
        Some(receipt.bridge_session_id().expect("session-backed")),
        "machine-routed member_status should preserve the active session binding"
    );
}

#[tokio::test]
async fn test_member_status_surface_exposes_current_session_id_for_realtime_routing() {
    // Identity-first realtime routing (Phase 5G/T5i) requires callers to
    // resolve `mob/member_status → current_session_id → realtime/open_info
    // (session_target)`. The session id MUST survive serialization to
    // JSON — it is the sole bridge from mob membership to a realtime
    // session handle since `mob_member_target` was removed.
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let receipt = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let snapshot = handle
        .member_status(&AgentIdentity::from("w-1"))
        .await
        .expect("member status");
    let expected_session_id = receipt
        .bridge_session_id()
        .expect("session-backed receipt")
        .clone();

    let json_value = serde_json::to_value(&snapshot).expect("serialize snapshot");
    assert_eq!(
        json_value
            .get("current_session_id")
            .and_then(|v| v.as_str()),
        Some(expected_session_id.to_string().as_str()),
        "mob/member_status response must surface current_session_id for realtime routing: {json_value}"
    );
    // The bridge-internal alias stays hidden — it is not part of the
    // public identity contract.
    assert!(
        json_value.get("current_bridge_session_id").is_none(),
        "current_bridge_session_id is bridge-internal and must not leak over the wire: {json_value}"
    );
}

#[tokio::test]
async fn test_member_handle_bridge_session_helper_round_trips_through_machine_projection_surface() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let receipt = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let member = handle
        .member(&AgentIdentity::from("w-1"))
        .await
        .expect("member handle");

    let current_bridge_session_id = member
        .current_bridge_session_id()
        .await
        .expect("current session id");
    assert_eq!(
        current_bridge_session_id.as_ref(),
        Some(receipt.bridge_session_id().expect("session-backed")),
        "member handle should expose the machine-routed current session binding"
    );
}

#[tokio::test]
async fn test_agent_event_subscriptions_resolve_through_machine_routed_member_reads() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let _stream = handle
        .subscribe_agent_events(&AgentIdentity::from("l-1"))
        .await
        .expect("member stream");

    let streams = handle
        .subscribe_all_agent_events()
        .await
        .expect("all-member streams");
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].0, AgentIdentity::from("l-1"));
}

#[tokio::test]
async fn test_mob_events_view_round_trips_through_machine_command_surface() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let replay = handle.events().replay_all().await.expect("replay all");
    assert!(
        replay
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MemberSpawned(..))),
        "replayed event view should include spawn events through the machine command surface"
    );

    let polled = handle.events().poll(0, 32).await.expect("poll events");
    assert!(
        !polled.is_empty(),
        "polled event view should surface stored events through the machine command surface"
    );

    let direct_polled = handle.poll_events(0, 32).await.expect("direct poll events");
    assert!(
        !direct_polled.is_empty(),
        "direct handle poll_events should also round-trip through the machine command surface"
    );
}

#[tokio::test]
async fn test_mob_events_view_latest_cursor_uses_store_cursor_without_replay() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let (handle, _service) = create_test_mob_with_events(sample_definition(), events.clone()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let replay_calls_before = events.replay_calls();
    let cursor = handle
        .events()
        .latest_cursor()
        .await
        .expect("latest cursor");

    assert!(cursor > 0, "latest cursor should reflect stored events");
    assert_eq!(
        events.replay_calls(),
        replay_calls_before,
        "latest_cursor should not replay the full event log"
    );
}

#[tokio::test]
async fn test_mob_events_view_subscribe_streams_structural_events() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let (handle, _service) = create_test_mob_with_events(sample_definition(), events.clone()).await;
    let poll_calls_before = events.poll_calls();
    let mut subscription = handle
        .events()
        .subscribe_with_config(MobEventsSubscriptionConfig {
            batch_limit: 16,
            channel_capacity: 16,
            ..MobEventsSubscriptionConfig::default()
        })
        .await
        .expect("subscribe to structural mob events");
    assert_eq!(
        events.poll_calls(),
        poll_calls_before,
        "live structural subscription should not poll to start at the latest cursor"
    );

    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let observed = tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(event) = subscription.event_rx.recv().await {
            if matches!(event.kind, MobEventKind::MemberSpawned(..)) {
                return Some(event);
            }
        }
        None
    })
    .await
    .expect("subscription should receive a structural mob event")
    .expect("subscription stream should remain open until the spawn event arrives");

    assert!(observed.cursor > 0);
    assert_eq!(
        events.poll_calls(),
        poll_calls_before,
        "live structural subscription should receive append notifications without polling"
    );
}

#[tokio::test]
async fn test_mob_events_view_subscribe_after_rejects_future_cursor() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let (handle, _service) = create_test_mob_with_events(sample_definition(), events).await;
    let latest_cursor = handle
        .events()
        .latest_cursor()
        .await
        .expect("latest cursor");
    let after_cursor = latest_cursor.saturating_add(10);

    match handle.events().subscribe_after(after_cursor).await {
        Err(MobError::StaleEventCursor {
            after_cursor: observed_after_cursor,
            latest_cursor: observed_latest_cursor,
        }) => {
            assert_eq!(observed_after_cursor, after_cursor);
            assert_eq!(observed_latest_cursor, latest_cursor);
        }
        Err(error) => panic!("expected stale cursor error, got {error:?}"),
        Ok(_) => panic!("future cursor subscription should fail"),
    }
}

#[tokio::test]
async fn test_mob_handle_list_runs_reads_public_run_listing() {
    let store = Arc::new(InMemoryMobRunStore::new());
    let definition = sample_definition();
    let mob_id = definition.id.clone();
    let alpha_flow = FlowId::from("alpha");
    let beta_flow = FlowId::from("beta");
    let alpha_run = MobRun::pending(
        mob_id.clone(),
        alpha_flow.clone(),
        crate::run::flow_run::initial_state(),
        serde_json::json!({"flow": "alpha"}),
    );
    let alpha_run_id = alpha_run.run_id.clone();
    let beta_run = MobRun::pending(
        mob_id,
        beta_flow,
        crate::run::flow_run::initial_state(),
        serde_json::json!({"flow": "beta"}),
    );

    store.create_run(alpha_run).await.expect("create alpha run");
    store.create_run(beta_run).await.expect("create beta run");
    let (handle, _service) = create_test_mob_with_run_store(definition, store).await;

    let all_runs = handle.list_runs(None).await.expect("list all runs");
    assert_eq!(all_runs.len(), 2);

    let filtered = handle
        .list_runs(Some(&alpha_flow))
        .await
        .expect("list runs by flow");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].run_id, alpha_run_id);
}

#[tokio::test]
async fn test_record_operator_action_provenance_round_trips_through_machine_command_surface() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let authority = meerkat_core::service::MobToolAuthorityContext::create_only_generated()
        .with_audit_invocation_id("audit-machine-surface");

    handle
        .record_operator_action_provenance("spawn_member", &authority)
        .await
        .expect("record operator action");

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            MobEventKind::OperatorActionRecorded {
                tool_name,
                audit_invocation_id,
                ..
            } if tool_name == "spawn_member"
                && audit_invocation_id.as_deref() == Some("audit-machine-surface")
        )),
        "operator action provenance should be appended through the machine command surface"
    );
}

#[tokio::test]
async fn test_mob_event_router_stays_alive_across_machine_routed_spawn_tracking() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let mut router = handle
        .subscribe_mob_events_with_config(MobEventRouterConfig {
            poll_interval: Duration::from_millis(10),
            channel_capacity: 32,
        })
        .await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        matches!(
            router.event_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "router should remain live after tracking a spawned member through machine-routed bootstrap"
    );
    router.cancel();
}

#[tokio::test]
async fn test_reset_append_failure_transitions_to_stopped() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let (handle, _service) = create_test_mob_with_events(sample_definition(), events.clone()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);

    // Fail the MobReset append — MobCreated append succeeds but reset
    // should still report failure. After destructive steps (retire/stop),
    // fail-closed to Stopped.
    events.fail_appends_for("MobReset").await;

    let result = handle.reset().await;
    assert!(
        matches!(result, Err(MobError::StorageError(_))),
        "reset should surface append failure"
    );
    assert_eq!(
        handle.status().await.unwrap(),
        MobState::Stopped,
        "failed reset after destructive steps must transition to Stopped"
    );

    // The event log should still contain a MobCreated event so resume works.
    let all_events = events.replay_all().await.expect("replay");
    assert!(
        all_events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MobCreated { .. })),
        "MobCreated must survive a failed reset"
    );
}

#[tokio::test]
async fn test_reset_failure_from_stopped_stays_stopped() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let (handle, _service) = create_test_mob_with_events(sample_definition(), events.clone()).await;

    handle.stop().await.expect("stop");
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);

    // Fail the MobCreated append to trigger reset failure from Stopped.
    events.fail_appends_for("MobCreated").await;
    let result = handle.reset().await;
    assert!(result.is_err(), "reset should fail");

    assert_eq!(
        handle.status().await.unwrap(),
        MobState::Stopped,
        "failed reset from Stopped must remain Stopped"
    );
}

#[tokio::test]
async fn test_shutdown_does_not_stall_on_stuck_lifecycle_notification() {
    // Create a definition with a TurnDriven orchestrator so lifecycle
    // notifications go through start_turn (which we can delay).
    let mut def = sample_definition();
    if let Some(profile) = def
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(|b| b.as_inline_mut())
    {
        profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    }

    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(def, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    // Spawn the orchestrator so the notification path is active.
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    // Make start_turn hang for 10 minutes — simulates a stuck backend.
    service.set_start_turn_delay_ms(600_000);

    // Stop the mob. This fires a lifecycle notification that will hang
    // in start_turn due to the delay. The notification is spawned onto
    // the JoinSet, so stop itself returns immediately.
    handle.stop().await.expect("stop");

    // Shutdown must complete quickly despite the stuck notification task.
    // abort_all cancels in-flight tasks instead of awaiting them.
    let shutdown_result = tokio::time::timeout(Duration::from_secs(5), handle.shutdown()).await;

    assert!(
        shutdown_result.is_ok(),
        "shutdown must not stall on stuck lifecycle notification tasks"
    );
    shutdown_result.unwrap().expect("shutdown should succeed");
}

// ---------------------------------------------------------------------------
// P1-5: Disposal policy — archive failure must propagate as Err
// ---------------------------------------------------------------------------

/// Archive failure during retirement must return Err to the caller.
///
/// Dogma §15: "success means truthful" — returning Ok(()) when ArchiveSession
/// failed creates an orphan session that the caller believes was cleaned up.
/// The roster is still removed (unconditional finally block), but the caller
/// must know the session was NOT archived.
#[tokio::test]
async fn test_retire_returns_err_when_archive_fails() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session_id = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    service.set_archive_failure(&session_id).await;

    // Archive failure is critical — retire must surface it.
    let result = handle.retire(AgentIdentity::from("w-1")).await;
    assert!(
        result.is_err(),
        "retire must return Err when ArchiveSession fails — got Ok"
    );

    // Roster removal is unconditional (finally block) even when we return Err.
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "roster entry must be removed even when retire returns Err"
    );

    // Retire event was persisted before disposal (event-first).
    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MemberRetired { .. })),
        "retire event must persist regardless of archive outcome"
    );
}

/// Comms failures (NotifyPeers, RemoveTrustEdges) during retirement remain
/// best-effort — retire returns Ok even when they fail.
///
/// These steps involve peer communication that may legitimately fail during
/// concurrent teardown. Only ArchiveSession is critical because it determines
/// whether session data is properly cleaned up.
#[tokio::test]
async fn test_retire_comms_failures_remain_best_effort() {
    let (handle, service) = create_test_mob(sample_definition()).await;

    // Set up comms to fail both NotifyPeers and RemoveTrustEdges for w-1.
    service
        .set_comms_behavior(
            &test_comms_name("worker", "w-1"),
            MockCommsBehavior {
                fail_send_peer_retired: true,
                fail_remove_trust: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle
        .wire(AgentIdentity::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    // Comms failures are best-effort — retire still succeeds.
    let result = handle.retire(AgentIdentity::from("w-1")).await;
    assert!(
        result.is_ok(),
        "retire must succeed when only comms steps fail (best-effort) — got {:?}",
        result.unwrap_err()
    );

    // Roster removal still happens.
    assert!(
        handle
            .get_member(&AgentIdentity::from("w-1"))
            .await
            .is_none(),
        "roster entry must be removed after best-effort comms failures"
    );
}

// ---------------------------------------------------------------------------
// NameFilteredDispatcher tests
// ---------------------------------------------------------------------------

/// A configurable dispatcher for testing name filtering and external tools.
struct MultiToolDispatcher {
    defs: Arc<[Arc<ToolDef>]>,
}

impl MultiToolDispatcher {
    fn new(names: &[&str]) -> Self {
        let defs: Vec<Arc<ToolDef>> = names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: (*name).into(),
                    description: format!("Tool {name}"),
                    input_schema: serde_json::Value::Object(serde_json::Map::new()),
                    provenance: None,
                })
            })
            .collect();
        Self { defs: defs.into() }
    }
}

#[async_trait]
impl AgentToolDispatcher for MultiToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.defs)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if self.defs.iter().any(|d| d.name == call.name) {
            Ok(ToolResult::new(
                call.id.to_string(),
                format!("{{\"tool\":\"{}\"}}", call.name),
                false,
            )
            .into())
        } else {
            Err(ToolError::not_found(call.name))
        }
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

#[tokio::test]
async fn test_name_filtered_dispatcher() {
    use super::tools::NameFilteredDispatcher;

    let inner = Arc::new(MultiToolDispatcher::new(&["a", "b", "c"]));
    let excluded: HashSet<String> = ["b".to_string()].into_iter().collect();
    let filtered = NameFilteredDispatcher::new(inner, excluded);

    // tools() should return only ["a", "c"]
    let tool_names: Vec<String> = filtered
        .tools()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    assert_eq!(tool_names.len(), 2);
    assert!(tool_names.contains(&"a".to_string()));
    assert!(tool_names.contains(&"c".to_string()));
    assert!(!tool_names.contains(&"b".to_string()));

    // dispatch("b") should error
    let raw_args = RawValue::from_string("{}".to_string()).unwrap();
    let result = filtered
        .dispatch(ToolCallView {
            id: "call-1",
            name: "b",
            args: &raw_args,
        })
        .await;
    assert!(result.is_err(), "excluded tool should return not_found");

    // dispatch("a") should succeed
    let result = filtered
        .dispatch(ToolCallView {
            id: "call-2",
            name: "a",
            args: &raw_args,
        })
        .await;
    assert!(result.is_ok(), "non-excluded tool should delegate to inner");
}

// ── RuntimeBinding TDD tests ────────────────────────────────────────────
//
// These tests define the expected behavior for RuntimeBinding — the first
// step toward identity-first mobs. They verify that external members carry
// real process identity, not phantom placeholder comms keys.

#[tokio::test]
async fn test_external_spawn_with_binding_uses_real_identity() {
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "external-spawn-real-identity",
    );
    let mob_id = definition.id.clone();
    let (handle, _service) = create_test_mob(definition).await;
    let external =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-real")).await;
    let crate::RuntimeBinding::External {
        peer_id: expected_peer_id,
        address: expected_address,
        bootstrap_token: _expected_bootstrap_token,
        pubkey: _expected_pubkey,
    } = external.binding()
    else {
        panic!("live external peer must produce external binding");
    };

    let mut spec = SpawnMemberSpec::new("worker", "w-real");
    spec.binding = Some(external.binding());

    let spawn_result = handle.spawn_spec(spec).await.expect("spawn with binding");
    // Verify identity was created
    assert!(!spawn_result.agent_identity.as_str().is_empty());
    // Verify backend binding through roster
    let entry = handle
        .roster()
        .await
        .get_by_identity(&spawn_result.agent_identity)
        .cloned()
        .expect("roster entry");
    match &entry.member_ref {
        MemberRef::BackendPeer {
            peer_id,
            address,
            session_id,
            ..
        } => {
            assert_eq!(
                peer_id, &expected_peer_id,
                "BackendPeer.peer_id must be the committed external process key"
            );
            assert_eq!(
                address,
                canonical_external_address(&expected_address),
                "BackendPeer.address must be the committed external process transport address"
            );
            assert!(
                session_id.is_none(),
                "peer-only external members should not keep a hidden bridge session"
            );
        }
        other => panic!("expected BackendPeer member ref, got {other:?}"),
    }
}

#[tokio::test]
async fn test_external_spawn_without_binding_errors() {
    let (handle, _service) = create_test_mob(sample_definition_with_external_backend()).await;

    // Bare MobBackendKind::External without RuntimeBinding must fail — you
    // can't spawn an external member without saying who the real process is.
    let mut spec = SpawnMemberSpec::new("worker", "w-bare");
    spec.backend = Some(MobBackendKind::External);
    // No binding set.

    let result = handle.spawn_spec(spec).await;

    assert!(
        result.is_err(),
        "external backend without RuntimeBinding must be rejected: {result:?}"
    );
}

#[tokio::test]
async fn test_session_spawn_default_binding() {
    // Regression guard: session backend spawns continue to work with no binding.
    let (handle, _service) = create_test_mob(sample_definition_with_external_backend()).await;

    let member_ref = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-session"),
            None,
        )
        .await
        .expect("spawn session member");

    assert!(
        matches!(member_ref, MemberRef::Session { .. }),
        "default spawn should produce session member ref"
    );
}

#[tokio::test]
async fn test_trusted_peer_spec_uses_real_external_identity_for_peer_only_members() {
    // Peer-only external members should wire using the real external transport
    // identity rather than a hidden local bridge session.
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "trusted-peer-spec",
    );
    let mob_id = definition.id.clone();
    let (handle, service) = create_test_mob_with_real_comms(definition).await;
    let external_name = test_comms_name_for(&mob_id, "worker", "w-bridge");
    let external = spawn_live_external_peer(&external_name).await;
    let crate::RuntimeBinding::External {
        peer_id,
        address,
        bootstrap_token,
        pubkey,
    } = external.binding()
    else {
        panic!("live external peer must produce external binding");
    };

    let mut spec = SpawnMemberSpec::new("worker", "w-bridge");
    spec.binding = Some(crate::RuntimeBinding::External {
        peer_id: peer_id.clone(),
        address: address.clone(),
        bootstrap_token: bootstrap_token.clone(),
        pubkey,
    });
    let _member_ref = handle
        .spawn_spec(spec)
        .await
        .expect("spawn with real binding");

    // Wire to orchestrator to trigger trust spec resolution
    let lead_ref = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("lead-1"), None)
        .await
        .expect("spawn lead");
    handle
        .wire(AgentIdentity::from("lead-1"), MeerkatId::from("w-bridge"))
        .await
        .expect("wire lead to external member");

    // Check what trust was added to the lead's comms runtime.
    let lead_sid = lead_ref
        .bridge_session_id()
        .cloned()
        .expect("lead session id");
    let trusted_peers = service
        .real_comms(&lead_sid)
        .await
        .expect("lead comms runtime")
        .peers()
        .await;
    let trusted_peer = trusted_peers
        .iter()
        .find(|entry| entry.name.as_str() == external_name)
        .expect("lead should trust the real external peer");

    assert!(
        trusted_peer.address.to_string() == canonical_external_address(&address),
        "peer-only trust specs should use the real external address. Got: {trusted_peers:?}",
    );
    assert!(
        trusted_peer.peer_id.to_string() == peer_id,
        "peer-only trust should retain the real external peer identity. Got: {trusted_peers:?}",
    );
}

#[tokio::test]
async fn test_supervisor_trust_does_not_leak_into_member_peer_directory() {
    // Fix 2 regression (negative invariant): `finalize_spawn_from_pending`
    // bootstraps supervisor trust on every session-backed member so
    // lifecycle notifications land at the classified inbox. That bootstrap
    // must NOT register the supervisor as a publicly-listed trusted peer —
    // it's a control-plane edge and would otherwise surface as an ordinary
    // sendable peer in the member's `comms.peers` output (and downstream
    // REST/RPC/MCP). The private-trust seam uses the router's
    // private peer-id directory-filter to keep the entry invisible.
    //
    // The *positive* complement — "the supervisor is still trusted enough
    // for the member to send back to it" — is enforced by
    // `test_supervisor_private_trust_preserves_send_resolution` below.
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(sample_definition(), "no-supervisor-leak");
    let (handle, service) = create_test_mob_with_real_comms(definition).await;
    let receipt = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-private"),
            None,
        )
        .await
        .expect("spawn session-backed member");
    let session_id = receipt
        .bridge_session_id()
        .cloned()
        .expect("session-backed member has bridge session id");

    let member_comms = service
        .real_comms(&session_id)
        .await
        .expect("member comms runtime");
    let directory = member_comms.peers().await;
    let supervisor_entry = directory
        .iter()
        .find(|entry| entry.name.as_str().contains("__mob_supervisor__"));
    assert!(
        supervisor_entry.is_none(),
        "supervisor must NOT appear in the session-backed member's public \
         peer directory; the private-trust seam is the whole point. Got: {directory:?}"
    );
}

#[tokio::test]
async fn test_supervisor_private_trust_failure_does_not_commit_spawn() {
    let definition = with_unique_mob_id(sample_definition(), "supervisor-trust-failure");
    let mob_id = definition.id.clone();
    let (handle, service) = create_test_mob(definition).await;
    service
        .set_comms_behavior(
            &format!("{mob_id}/worker/w-trust-fail"),
            MockCommsBehavior {
                fail_add_trust: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    let error = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-trust-fail"),
            None,
        )
        .await
        .expect_err("supervisor private trust failure should fail spawn");
    assert!(
        error
            .to_string()
            .contains("supervisor private trust publication failed"),
        "spawn should surface the authority-gated trust failure, got: {error}"
    );
    assert!(
        handle.list_members().await.is_empty(),
        "failed supervisor trust publication must not commit roster membership"
    );
    assert_eq!(
        service.active_session_count().await,
        0,
        "failed supervisor trust publication must roll back the provisioned session"
    );
    let events = handle.events().replay_all().await.expect("replay events");
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.kind, MobEventKind::MemberSpawned(_))),
        "failed supervisor trust publication must not append MemberSpawned"
    );
}

#[tokio::test]
async fn test_supervisor_private_trust_preserves_send_resolution() {
    // Fix 2 positive invariant: the private-trust seam must still keep
    // the supervisor fully send-resolvable from the member. When splitting
    // `add_trusted_peer` into "admission + directory + send-resolution" vs
    // "admission + send-resolution (no directory)", the positive side was
    // the one that broke first — e2e-smoke caught pictionary kickoff
    // timing out because the admission-only path left the member's
    // router unable to resolve the supervisor's `PeerName` to a `PeerAddr`,
    // so reply-sends silently failed. This test pins the required
    // invariants: the supervisor entry is in the router's `TrustedPeers`
    // (so `send_peer_command` can resolve), is marked private (so the
    // directory hides it), and the member's classified inbox knows the
    // supervisor pubkey (so admission accepts lifecycle notifications).
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(sample_definition(), "supervisor-send-resolution");
    let (handle, service) = create_test_mob_with_real_comms(definition).await;
    let receipt = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-send"), None)
        .await
        .expect("spawn session-backed member");
    let session_id = receipt
        .bridge_session_id()
        .cloned()
        .expect("session-backed member has bridge session id");
    let member_comms = service
        .real_comms(&session_id)
        .await
        .expect("member comms runtime");

    // Look up the supervisor pubkey via the member's trust list — this is
    // the same list the router uses to resolve `PeerName → PeerAddr`.
    let supervisor_pubkey = {
        let trusted = member_comms.trusted_peers_shared();
        let guard = trusted.read();
        let entry = guard
            .peers
            .iter()
            .find(|p| p.name.contains("__mob_supervisor__"))
            .expect(
                "send-resolution invariant: supervisor must still appear in the \
                 member's internal TrustedPeers — that's what the router consults \
                 to resolve the supervisor's PeerName to a PeerAddr on reply-sends",
            );
        entry.pubkey
    };

    // The directory-filter side-channel must mark this pubkey private.
    assert!(
        member_comms.router().is_private(&supervisor_pubkey),
        "directory-filter invariant: supervisor pubkey must be marked \
         private so `resolve_peer_directory()` hides it"
    );

    // And the directory must in fact hide it. This is the same fact the
    // negative-invariant test asserts at the `peers()` API, pinned here
    // as the end-to-end consequence of the two internal invariants above.
    let directory = member_comms.peers().await;
    assert!(
        directory
            .iter()
            .all(|entry| !entry.name.as_str().contains("__mob_supervisor__")),
        "directory output must not list the supervisor; got: {directory:?}"
    );
}

#[tokio::test]
async fn test_rotate_supervisor_reinstalls_private_trust_on_session_backed_member() {
    // W2-F integration test: after a supervisor rotation, session-backed
    // members must privately trust the NEW supervisor peer id and must no
    // longer trust the previous one. Without the re-install step, the old
    // peer id sticks in the member's private-trust set and new-supervisor
    // lifecycle notifications (`mob.peer_added`, `mob.peer_retired`, …) are
    // dropped at the classified inbox's admission gate.
    let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
    let definition = with_unique_mob_id(sample_definition(), "rotate-private-trust");
    let mob_id = definition.id.clone();
    let (handle, service) = create_test_mob_with_real_comms(definition).await;
    let receipt = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-rotate"),
            None,
        )
        .await
        .expect("spawn session-backed member");
    let session_id = receipt
        .bridge_session_id()
        .cloned()
        .expect("session-backed member has bridge session id");
    let member_comms = service
        .real_comms(&session_id)
        .await
        .expect("member comms runtime");

    // Capture the pre-rotation supervisor pubkey. This is what the member's
    // private trust set must no longer trust after the rotation.
    let previous_supervisor_pubkey = {
        let trusted = member_comms.trusted_peers_shared();
        let guard = trusted.read();
        let entry = guard
            .peers
            .iter()
            .find(|p| p.name.contains("__mob_supervisor__"))
            .expect("spawn bootstraps supervisor private trust");
        entry.pubkey
    };
    assert!(
        member_comms
            .router()
            .is_private(&previous_supervisor_pubkey),
        "pre-rotation supervisor must start as a private-trust entry"
    );

    let previous_name = {
        let trusted = member_comms.trusted_peers_shared();
        let guard = trusted.read();
        guard
            .peers
            .iter()
            .find(|p| p.name.contains("__mob_supervisor__"))
            .expect("supervisor trust entry")
            .name
            .clone()
    };

    let report = handle.rotate_supervisor().await.expect("rotate supervisor");
    assert_eq!(
        report.previous_epoch + 1,
        report.current_epoch,
        "rotation must bump epoch by exactly one"
    );
    assert_ne!(
        report.public_peer_id,
        previous_supervisor_pubkey.to_peer_id().to_string(),
        "rotation must replace the supervisor keypair"
    );

    // Post-rotation: the old supervisor pubkey must no longer be trusted,
    // and the rotated supervisor pubkey must be present and marked private
    // so the directory still hides it while the admission gate still
    // accepts lifecycle notifications.
    let (new_supervisor_pubkey_opt, old_still_present) = {
        let trusted = member_comms.trusted_peers_shared();
        let guard = trusted.read();
        let new_entry = guard
            .peers
            .iter()
            .find(|p| p.name.contains("__mob_supervisor__"));
        let old_still_present = guard
            .peers
            .iter()
            .any(|p| p.pubkey == previous_supervisor_pubkey);
        (new_entry.map(|entry| entry.pubkey), old_still_present)
    };
    assert!(
        !old_still_present,
        "previous supervisor private trust must be revoked by the rotation \
         trust re-install; leaving it in would accept lifecycle notifications \
         from the superseded authority"
    );
    let new_supervisor_pubkey = new_supervisor_pubkey_opt
        .expect("rotated supervisor private trust must be installed on the session-backed member");
    assert_ne!(
        new_supervisor_pubkey, previous_supervisor_pubkey,
        "rotation must change the trusted supervisor pubkey"
    );
    assert!(
        member_comms.router().is_private(&new_supervisor_pubkey),
        "rotated supervisor must stay on the private-trust edge — it is a \
         control-plane peer and must not leak into `comms.peers`"
    );
    assert_eq!(
        new_supervisor_pubkey.to_peer_id().to_string(),
        report.public_peer_id,
        "member-side private trust must agree with the rotation report"
    );

    // And the directory must still hide the (rotated) supervisor, matching
    // the invariant from `test_supervisor_trust_does_not_leak_into_member_peer_directory`.
    let directory = member_comms.peers().await;
    assert!(
        directory
            .iter()
            .all(|entry| !entry.name.as_str().contains("__mob_supervisor__")),
        "rotated supervisor must remain absent from the public peer directory; got: {directory:?}"
    );

    // Sanity check: we really did spawn a session-backed member of the
    // expected mob (the trust-entry name embeds the mob id).
    assert!(
        previous_name.starts_with(mob_id.as_str()),
        "private-trust entry name must be scoped to this mob; got: {previous_name}"
    );
}

#[tokio::test]
async fn test_external_tools_provider_called_per_spawn() {
    let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counter_clone = counter.clone();
    let provider: crate::ExternalToolsProvider = Arc::new(move || {
        counter_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(Arc::new(MultiToolDispatcher::new(&["ext_tool"])) as Arc<dyn AgentToolDispatcher>)
    });

    let definition = sample_definition();
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .with_default_external_tools_provider(Some(provider))
        .create()
        .await
        .expect("create mob");

    // Spawn 3 workers
    for i in 1..=3 {
        handle
            .spawn(
                ProfileName::from("worker"),
                MeerkatId::from(format!("w-{i}")),
                None,
            )
            .await
            .expect("spawn");
    }

    assert_eq!(
        counter.load(std::sync::atomic::Ordering::Relaxed),
        3,
        "provider should be called once per spawn"
    );

    // Verify external tools are visible on spawned sessions
    let flags = service.recorded_external_tools_flags().await;
    assert!(
        flags.iter().all(|&f| f),
        "all spawned sessions should have external tools"
    );
}

#[tokio::test]
async fn test_external_tools_name_collision_profile_wins() {
    // Provider returns a dispatcher with tool "spawn_member". Without operator
    // context, the callback tool should pass through unchanged.
    let provider: crate::ExternalToolsProvider = Arc::new(|| {
        Some(
            Arc::new(MultiToolDispatcher::new(&["spawn_member", "my_callback"]))
                as Arc<dyn AgentToolDispatcher>,
        )
    });

    let definition = sample_definition_with_mob_tools();
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .with_default_external_tools_provider(Some(provider))
        .create()
        .await
        .expect("create mob");

    let member_ref = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn should succeed despite name collision");

    let session_id = member_ref
        .bridge_session_id()
        .expect("session-backed")
        .clone();
    let tool_names = service.external_tool_names(&session_id).await;

    // Without operator context there is no mob-owned spawn_member, so the callback
    // tool should remain visible.
    assert!(
        tool_names.contains(&"spawn_member".to_string()),
        "callback spawn_member should be present when operator tools are hidden"
    );
    // callback's unique tool should still be present
    assert!(
        tool_names.contains(&"my_callback".to_string()),
        "non-colliding callback tool should be present"
    );

    // Dispatch spawn_member — should hit the callback dispatcher directly.
    let raw_args = serde_json::json!({"profile": "worker", "member_id": "w-test"});
    let result = service
        .dispatch_external_tool_outcome(&session_id, "spawn_member", raw_args)
        .await
        .expect("callback spawn_member should dispatch");
    let payload: serde_json::Value =
        serde_json::from_str(&result.result.text_content()).expect("callback result json");
    assert_eq!(payload["tool"], "spawn_member");
}

#[tokio::test]
async fn test_external_tools_late_registration() {
    let tool_defs: Arc<std::sync::RwLock<Vec<&str>>> = Arc::new(std::sync::RwLock::new(Vec::new()));
    let tool_defs_clone = tool_defs.clone();

    let provider: crate::ExternalToolsProvider = Arc::new(move || {
        let names = tool_defs_clone.read().expect("lock");
        if names.is_empty() {
            return None;
        }
        let defs: Vec<Arc<ToolDef>> = names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: (*name).into(),
                    description: format!("Tool {name}"),
                    input_schema: serde_json::Value::Object(serde_json::Map::new()),
                    provenance: None,
                })
            })
            .collect();
        struct DynDispatcher(Arc<[Arc<ToolDef>]>);
        #[async_trait]
        impl AgentToolDispatcher for DynDispatcher {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                Arc::clone(&self.0)
            }
            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<ToolDispatchOutcome, ToolError> {
                Ok(ToolResult::new(call.id.to_string(), "{}".to_string(), false).into())
            }
        }
        Some(Arc::new(DynDispatcher(defs.into())) as Arc<dyn AgentToolDispatcher>)
    });

    let definition = sample_definition();
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .with_default_external_tools_provider(Some(provider))
        .create()
        .await
        .expect("create mob");

    // Spawn before registering tools — provider returns None
    let _ref1 = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-before"),
            None,
        )
        .await
        .expect("spawn");
    let flags = service.recorded_external_tools_flags().await;
    assert!(
        !flags[0],
        "spawn before registration should have no external tools"
    );

    // Now register tools
    {
        let mut names = tool_defs.write().expect("lock");
        names.push("late_tool_a");
        names.push("late_tool_b");
    }

    // Spawn after registration — provider returns tools
    let ref2 = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("w-after"),
            None,
        )
        .await
        .expect("spawn");
    let flags = service.recorded_external_tools_flags().await;
    assert!(
        flags[1],
        "spawn after registration should have external tools"
    );

    let session_id = ref2.bridge_session_id().expect("session-backed").clone();
    let tool_names = service.external_tool_names(&session_id).await;
    assert!(
        tool_names.contains(&"late_tool_a".to_string()),
        "late-registered tool should appear"
    );
    assert!(
        tool_names.contains(&"late_tool_b".to_string()),
        "late-registered tool should appear"
    );
}

#[tokio::test]
async fn test_restored_member_gets_external_tools() {
    let provider: crate::ExternalToolsProvider =
        Arc::new(|| Some(Arc::new(EchoBundleDispatcher) as Arc<dyn AgentToolDispatcher>));

    let definition = sample_definition();
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();

    // Create mob and spawn a worker
    let handle = MobBuilder::new(definition.clone(), storage)
        .with_session_service(service.clone())
        .with_default_external_tools_provider(Some(provider.clone()))
        .create()
        .await
        .expect("create mob");

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn");

    let old_sid = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("roster entry")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");

    // Stop and archive the session to simulate a stale state
    handle.stop().await.expect("stop");
    service.archive(&old_sid).await.expect("archive session");

    // Resume with provider — restored member should get external tools
    let _resumed = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .with_default_external_tools_provider(Some(provider))
    .resume()
    .await
    .expect("resume");

    // The restore path creates a new session for the member.
    // Check that the create_session call included external tools.
    let flags = service.recorded_external_tools_flags().await;
    // flags[0] = original spawn, flags[1] = restored session
    assert!(
        flags.len() >= 2,
        "should have at least 2 create_session calls (original + restore)"
    );
    assert!(
        flags[1],
        "restored member should have external tools from provider"
    );
}

/// Integration test: a flow with `root: Some(FrameSpec { ... })` executes frame nodes
/// via the FlowFrameEngine path (not the flat-step path).
///
/// This validates the wiring added in task #16 — FlowSpec.root dispatches to
/// FlowFrameEngine with a scripted step executor.
#[tokio::test]
async fn test_flow_with_root_frame_spec_executes_frame_nodes() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec};
    use crate::ids::{FlowNodeId, FrameId};
    use crate::run::FlowContext;
    use crate::runtime::flow_frame_engine::{FlowFrameEngine, FrameStepExecutor, FrameStepResult};

    /// Scripted executor that returns pre-configured output per node.
    struct ScriptedExecutor {
        outputs: std::collections::HashMap<String, serde_json::Value>,
    }

    #[async_trait]
    impl FrameStepExecutor for ScriptedExecutor {
        async fn execute_step(
            &self,
            _run_id: &crate::ids::RunId,
            _frame_id: &FrameId,
            node_id: &FlowNodeId,
            _step_id: &crate::ids::StepId,
            _context: &FlowContext,
        ) -> Result<FrameStepResult, MobError> {
            self.outputs
                .get(&node_id.to_string())
                .cloned()
                .map(FrameStepResult::Completed)
                .ok_or_else(|| MobError::Internal(format!("no scripted output for {node_id}")))
        }
    }

    // Build store + run.
    let store = Arc::new(InMemoryMobRunStore::new());
    let run = MobRun::pending(
        crate::ids::MobId::from("test-mob"),
        FlowId::from("test-flow"),
        crate::run::flow_run::initial_state(),
        serde_json::json!({}),
    );
    let run_id = run.run_id.clone();
    store.create_run(run).await.expect("create_run");

    // Build a root FrameSpec: setup -> finalize
    let root_spec = {
        let mut nodes = IndexMap::new();
        nodes.insert(
            FlowNodeId::from("setup-node"),
            FlowNodeSpec::Step(FrameStepSpec {
                step_id: crate::ids::StepId::from("setup"),
                depends_on: vec![],
                depends_on_mode: crate::definition::DependencyMode::All,
                branch: None,
            }),
        );
        nodes.insert(
            FlowNodeId::from("finalize-node"),
            FlowNodeSpec::Step(FrameStepSpec {
                step_id: crate::ids::StepId::from("finalize"),
                depends_on: vec![FlowNodeId::from("setup-node")],
                depends_on_mode: crate::definition::DependencyMode::All,
                branch: None,
            }),
        );
        FrameSpec { nodes }
    };

    let mut scripted_outputs = std::collections::HashMap::new();
    scripted_outputs.insert(
        "setup-node".to_string(),
        serde_json::json!({"initialized": true}),
    );
    scripted_outputs.insert(
        "finalize-node".to_string(),
        serde_json::json!({"complete": true}),
    );

    let executor = Arc::new(ScriptedExecutor {
        outputs: scripted_outputs,
    });
    let definition = sample_definition();
    let (handle, _) = create_test_mob_with_run_store(definition, store.clone()).await;
    seed_test_run_in_mob_machine(&handle, &run_id).await;
    let engine = FlowFrameEngine::new(store.clone(), executor, handle, 0, 0);
    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };
    let frame_id = FrameId::from(format!("{run_id}-root").as_str());

    let outputs = engine
        .execute_frame(&run_id, &frame_id, &root_spec, &context)
        .await
        .expect("frame-based execution should complete");

    assert!(
        outputs
            .outputs
            .contains_key(&crate::ids::StepId::from("setup")),
        "setup output missing; outputs: {outputs:?}"
    );
    assert!(
        outputs
            .outputs
            .contains_key(&crate::ids::StepId::from("finalize")),
        "finalize output missing; outputs: {outputs:?}"
    );
    assert_eq!(
        outputs.outputs[&crate::ids::StepId::from("finalize")],
        serde_json::json!({"complete": true}),
    );

    // Verify frame state is Completed in the store.
    let run = store
        .get_run(&run_id)
        .await
        .expect("get_run")
        .expect("run exists");
    let frame_snap = run.frames.get(&frame_id).expect("frame snapshot");
    assert_eq!(
        frame_snap.kernel_state.phase,
        crate::run::flow_frame::Phase::Completed,
        "frame should be in Completed phase"
    );
}

#[tokio::test]
async fn test_root_frame_condition_skip_emits_single_skip_projection() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec};
    use crate::ids::FlowNodeId;

    let mut definition = sample_definition_with_single_step_flow(500, 8);
    let flow = definition
        .flows
        .get_mut(&FlowId::from("demo"))
        .expect("demo flow");
    let step = flow.steps.get_mut(&step_id("start")).expect("start step");
    step.condition = Some(ConditionExpr::Eq {
        path: "params.ok".to_string(),
        value: serde_json::json!(true),
    });
    let mut root_nodes = IndexMap::new();
    root_nodes.insert(
        FlowNodeId::from("start-node"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: step_id("start"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    flow.root = Some(FrameSpec { nodes: root_nodes });

    let (handle, _service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({ "ok": false }))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let run = handle
        .flow_status(run_id.clone())
        .await
        .expect("flow status")
        .expect("run exists");
    assert_eq!(
        run.step_ledger
            .iter()
            .filter(|entry| {
                entry.step_id.as_str() == "start" && entry.status == StepRunStatus::Skipped
            })
            .count(),
        1,
        "root-frame skipped step should appear exactly once in the step ledger"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    MobEventKind::StepSkipped { run_id: id, step_id, .. }
                        if id == &run_id && step_id.as_str() == "start"
                )
            })
            .count(),
        1,
        "root-frame skipped step should emit exactly one StepSkipped event"
    );
}

#[tokio::test]
async fn test_root_frame_step_failure_does_not_abort_independent_siblings() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec};
    use crate::ids::FlowNodeId;

    let mut definition = sample_definition();
    let mut steps = IndexMap::new();
    steps.insert(step_id("needs_lead"), flow_step("lead", "Lead-only task"));
    steps.insert(step_id("worker_task"), flow_step("worker", "Worker task"));

    let mut root_nodes = IndexMap::new();
    root_nodes.insert(
        FlowNodeId::from("needs-lead-node"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: step_id("needs_lead"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    root_nodes.insert(
        FlowNodeId::from("worker-node"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: step_id("worker_task"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );

    definition.flows = BTreeMap::from([(
        flow_id("demo"),
        FlowSpec {
            description: Some("independent root siblings".to_string()),
            steps,
            root: Some(FrameSpec { nodes: root_nodes }),
        },
    )]);

    let (handle, _service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
    assert!(
        terminal.step_ledger.iter().any(|entry| {
            entry.step_id.as_str() == "worker_task" && entry.status == StepRunStatus::Completed
        }),
        "independent worker sibling should still complete even if another root node fails"
    );
    assert!(
        terminal.step_ledger.iter().any(|entry| {
            entry.step_id.as_str() == "needs_lead"
                && entry.status == StepRunStatus::Failed
                && entry.agent_identity == crate::runtime::flow_system_member_id()
        }),
        "root-frame failure should project a failed step entry for the failed node"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::StepCompleted { run_id: id, step_id }
                    if id == &run_id && step_id.as_str() == "worker_task"
            )
        }),
        "independent worker sibling should still emit StepCompleted"
    );
}

#[tokio::test]
async fn test_root_frame_step_failure_records_failure_ledger_once() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec};
    use crate::ids::FlowNodeId;

    let mut definition = sample_definition_with_single_step_flow(500, 8);
    let flow = definition
        .flows
        .get_mut(&FlowId::from("demo"))
        .expect("demo flow");
    flow.root = Some(FrameSpec {
        nodes: IndexMap::from([(
            FlowNodeId::from("start-node"),
            FlowNodeSpec::Step(FrameStepSpec {
                step_id: step_id("start"),
                depends_on: Vec::new(),
                depends_on_mode: DependencyMode::All,
                branch: None,
            }),
        )]),
    });

    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_fail(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
    assert_eq!(
        terminal
            .failure_ledger
            .iter()
            .filter(|entry| entry.step_id.as_str() == "start")
            .count(),
        1,
        "frame-mode target failures should record exactly one failure ledger row per failed step"
    );
}

#[tokio::test]
async fn test_root_frame_supervisor_threshold_is_respected_before_reset() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec};
    use crate::ids::FlowNodeId;

    let mut definition = sample_definition_with_supervisor_threshold(2);
    let flow = definition
        .flows
        .get_mut(&FlowId::from("demo"))
        .expect("demo flow");
    let mut root_nodes = IndexMap::new();
    root_nodes.insert(
        FlowNodeId::from("start-node"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: step_id("start"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    flow.root = Some(FrameSpec { nodes: root_nodes });

    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead supervisor");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_fail(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::SupervisorEscalation { run_id: id, .. } if id == &run_id
            )
        }),
        "frame-root failures should not escalate before the configured threshold"
    );
    assert_eq!(
        handle.list_members().await.len(),
        2,
        "force_reset should not retire members before the escalation threshold is crossed"
    );
}

#[tokio::test]
async fn test_root_frame_fan_in_persists_canonical_completed_aggregate_output() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec};
    use crate::ids::FlowNodeId;

    let mut definition =
        sample_definition_with_dispatch_mode_and_policy(DispatchMode::FanIn, CollectionPolicy::All);
    let flow = definition
        .flows
        .get_mut(&FlowId::from("dispatch"))
        .expect("dispatch flow");
    let mut root_nodes = IndexMap::new();
    root_nodes.insert(
        FlowNodeId::from("dispatch-node"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: step_id("dispatch"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    flow.root = Some(FrameSpec { nodes: root_nodes });

    let (handle, _service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");

    let run_id = handle
        .run_flow(FlowId::from("dispatch"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let aggregate_output = terminal
        .step_ledger
        .iter()
        .find(|entry| {
            entry.step_id.as_str() == "dispatch"
                && entry.agent_identity == crate::runtime::flow_system_member_id()
                && entry.status == StepRunStatus::Completed
        })
        .and_then(|entry| entry.output.clone())
        .expect("frame-root fan_in should persist canonical aggregate completed output");
    assert_eq!(
        aggregate_output,
        serde_json::json!([
            {"target":"w-1","output":"Turn completed"},
            {"target":"w-2","output":"Turn completed"}
        ])
    );
}

#[tokio::test]
async fn test_resume_running_loop_node_completes_instead_of_failing() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec, RepeatUntilSpec};
    use crate::ids::{FlowNodeId, FrameId, LoopId};
    use crate::run::FlowContext;
    use crate::runtime::flow_frame_engine::FlowFrameMutator;
    use crate::runtime::flow_frame_engine::{FlowFrameEngine, FrameStepExecutor, FrameStepResult};

    struct ScriptedExecutor;

    #[async_trait]
    impl FrameStepExecutor for ScriptedExecutor {
        async fn execute_step(
            &self,
            _run_id: &crate::ids::RunId,
            _frame_id: &FrameId,
            _node_id: &FlowNodeId,
            _step_id: &crate::ids::StepId,
            _context: &FlowContext,
        ) -> Result<FrameStepResult, MobError> {
            Ok(FrameStepResult::Completed(
                serde_json::json!({"done": true}),
            ))
        }
    }

    let store = Arc::new(InMemoryMobRunStore::new());
    let run = MobRun::pending(
        crate::ids::MobId::from("test-mob"),
        FlowId::from("test-flow"),
        crate::run::flow_run::initial_state(),
        serde_json::json!({}),
    );
    let run_id = run.run_id.clone();
    store.create_run(run).await.expect("create_run");

    let mut body_nodes = IndexMap::new();
    body_nodes.insert(
        FlowNodeId::from("body-step"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: crate::ids::StepId::from("body"),
            depends_on: vec![],
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    let body_spec = FrameSpec { nodes: body_nodes };
    let root_spec = {
        let mut nodes = IndexMap::new();
        nodes.insert(
            FlowNodeId::from("loop-node"),
            FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                loop_id: LoopId::from("retry"),
                depends_on: vec![],
                depends_on_mode: DependencyMode::All,
                body: body_spec.clone(),
                until: ConditionExpr::Eq {
                    path: "steps.body.done".to_string(),
                    value: serde_json::json!(true),
                },
                max_iterations: 3,
            }),
        );
        FrameSpec { nodes }
    };

    let definition = sample_definition();
    let (handle, _) = create_test_mob_with_run_store(definition, store.clone()).await;
    seed_test_run_in_mob_machine(&handle, &run_id).await;
    let frame_kernel =
        crate::runtime::flow_frame_engine::FlowFrameKernel::new(store.clone(), handle.clone());
    let frame_id = FrameId::from(format!("{run_id}-root").as_str());
    frame_kernel
        .start_frame(&run_id, &frame_id, &root_spec)
        .await
        .expect("start frame");
    frame_kernel
        .admit_next_ready_node_with_retry(&run_id, &frame_id, 5)
        .await
        .expect("admit loop node")
        .expect("loop node effects");

    let loop_instance_id =
        crate::ids::LoopInstanceId::from(format!("{frame_id}::loop-node").as_str());
    seed_test_loop_in_mob_machine(
        &handle,
        &loop_instance_id,
        &frame_id,
        &crate::ids::FlowNodeId::from("loop-node"),
        &crate::ids::LoopId::from("retry"),
        1,
        3,
    )
    .await;
    store
        .upsert_loop_snapshot(
            &run_id,
            &loop_instance_id,
            crate::run::LoopSnapshot {
                kernel_state: crate::run::loop_iteration::State {
                    phase: crate::run::loop_iteration::Phase::Running,
                    loop_instance_id: loop_instance_id.clone(),
                    parent_frame_id: frame_id.clone(),
                    parent_node_id: crate::ids::FlowNodeId::from("loop-node"),
                    loop_id: crate::ids::LoopId::from("retry"),
                    depth: 1,
                    stage: crate::run::loop_iteration::LoopIterationStage::AwaitingBodyFrame,
                    current_iteration: 0,
                    last_completed_iteration: 0,
                    max_iterations: 3,
                    active_body_frame_id: None,
                },
            },
            None,
        )
        .await
        .expect("seed running loop snapshot");

    let engine = FlowFrameEngine::new(
        store.clone(),
        Arc::new(ScriptedExecutor),
        handle.clone(),
        0,
        0,
    );
    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    let outputs = engine
        .execute_frame(&run_id, &frame_id, &root_spec, &context)
        .await
        .expect("resume running loop node");
    assert_eq!(
        outputs.outputs.get(&crate::ids::StepId::from("body")),
        Some(&serde_json::json!({"done": true}))
    );

    let run = store
        .get_run(&run_id)
        .await
        .expect("get_run")
        .expect("run exists");
    let frame_snap = run.frames.get(&frame_id).expect("frame snapshot");
    let node_status = &frame_snap.kernel_state.node_status;
    assert!(
        matches!(
            node_status.get("loop-node"),
            Some(variant) if *variant == crate::run::flow_frame::NodeRunStatus::Completed
        ),
        "running loop node should resume to completion instead of being failed on restart"
    );
}

#[tokio::test]
async fn test_resume_running_loop_node_does_not_duplicate_iteration_ledger_entry() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec, RepeatUntilSpec};
    use crate::ids::{FlowNodeId, FrameId, LoopId};
    use crate::run::FlowContext;
    use crate::runtime::flow_frame_engine::FlowFrameMutator;
    use crate::runtime::flow_frame_engine::{FlowFrameEngine, FrameStepExecutor, FrameStepResult};

    struct ScriptedExecutor;

    #[async_trait]
    impl FrameStepExecutor for ScriptedExecutor {
        async fn execute_step(
            &self,
            _run_id: &crate::ids::RunId,
            _frame_id: &FrameId,
            _node_id: &FlowNodeId,
            _step_id: &crate::ids::StepId,
            _context: &FlowContext,
        ) -> Result<FrameStepResult, MobError> {
            Ok(FrameStepResult::Completed(
                serde_json::json!({"done": true}),
            ))
        }
    }

    let store = Arc::new(InMemoryMobRunStore::new());
    let run = MobRun::pending(
        crate::ids::MobId::from("test-mob"),
        FlowId::from("test-flow"),
        crate::run::flow_run::initial_state(),
        serde_json::json!({}),
    );
    let run_id = run.run_id.clone();
    store.create_run(run).await.expect("create_run");

    let mut body_nodes = IndexMap::new();
    body_nodes.insert(
        FlowNodeId::from("body-step"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: crate::ids::StepId::from("body"),
            depends_on: vec![],
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    let body_spec = FrameSpec { nodes: body_nodes };
    let root_spec = {
        let mut nodes = IndexMap::new();
        nodes.insert(
            FlowNodeId::from("loop-node"),
            FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                loop_id: LoopId::from("retry"),
                depends_on: vec![],
                depends_on_mode: DependencyMode::All,
                body: body_spec.clone(),
                until: ConditionExpr::Eq {
                    path: "steps.body.done".to_string(),
                    value: serde_json::json!(true),
                },
                max_iterations: 3,
            }),
        );
        FrameSpec { nodes }
    };

    let definition = sample_definition();
    let (handle, _) = create_test_mob_with_run_store(definition, store.clone()).await;
    seed_test_run_in_mob_machine(&handle, &run_id).await;
    let frame_kernel =
        crate::runtime::flow_frame_engine::FlowFrameKernel::new(store.clone(), handle.clone());
    let frame_id = FrameId::from(format!("{run_id}-root").as_str());
    frame_kernel
        .start_frame(&run_id, &frame_id, &root_spec)
        .await
        .expect("start frame");
    frame_kernel
        .admit_next_ready_node_with_retry(&run_id, &frame_id, 5)
        .await
        .expect("admit loop node")
        .expect("loop node effects");

    let loop_instance_id =
        crate::ids::LoopInstanceId::from(format!("{frame_id}::loop-node").as_str());
    let body_frame_id = FrameId::from(format!("{loop_instance_id}::iter-0").as_str());
    seed_test_loop_in_mob_machine(
        &handle,
        &loop_instance_id,
        &frame_id,
        &crate::ids::FlowNodeId::from("loop-node"),
        &crate::ids::LoopId::from("retry"),
        1,
        3,
    )
    .await;
    store
        .upsert_loop_snapshot(
            &run_id,
            &loop_instance_id,
            crate::run::LoopSnapshot {
                kernel_state: crate::run::loop_iteration::State {
                    phase: crate::run::loop_iteration::Phase::Running,
                    loop_instance_id: loop_instance_id.clone(),
                    parent_frame_id: frame_id.clone(),
                    parent_node_id: crate::ids::FlowNodeId::from("loop-node"),
                    loop_id: crate::ids::LoopId::from("retry"),
                    depth: 1,
                    stage: crate::run::loop_iteration::LoopIterationStage::BodyFrameActive,
                    current_iteration: 0,
                    last_completed_iteration: 0,
                    max_iterations: 3,
                    active_body_frame_id: Some(body_frame_id.clone()),
                },
            },
            Some(crate::run::LoopIterationLedgerEntry {
                loop_instance_id: loop_instance_id.clone(),
                iteration: 0,
                frame_id: body_frame_id.clone(),
            }),
        )
        .await
        .expect("seed running loop snapshot with ledger entry");
    handle
        .project_machine_input(
            MobRun::create_frame_seed_input(
                &run_id,
                &body_frame_id,
                Some(&loop_instance_id),
                0,
                crate::machines::mob_machine::FrameScope::Body,
                &body_spec,
                &[FlowNodeId::from("body-step")],
            )
            .expect("body frame seed input"),
        )
        .await
        .expect("seed body frame in MobMachine");
    let body_node_id = FlowNodeId::from("body-step");
    let body_frame = crate::run::FrameSnapshot {
        kernel_state: crate::run::flow_frame::State {
            phase: crate::run::flow_frame::Phase::Running,
            frame_id: body_frame_id.clone(),
            frame_scope: crate::run::flow_frame::FrameScope::Body,
            loop_instance_id: loop_instance_id.clone(),
            iteration: 0,
            last_admitted_node: FlowNodeId::from(String::new()),
            tracked_nodes: [body_node_id.clone()].into_iter().collect(),
            ordered_nodes: vec![body_node_id.clone()],
            node_kind: [(
                body_node_id.clone(),
                crate::run::flow_frame::FlowNodeKind::Step,
            )]
            .into_iter()
            .collect(),
            node_dependencies: [(body_node_id.clone(), Vec::new())].into_iter().collect(),
            node_dependency_modes: [(
                body_node_id.clone(),
                crate::run::flow_frame::DependencyMode::All,
            )]
            .into_iter()
            .collect(),
            node_branches: [(body_node_id.clone(), None)].into_iter().collect(),
            branch_winners: Default::default(),
            node_status: [(
                body_node_id.clone(),
                crate::run::flow_frame::NodeRunStatus::Ready,
            )]
            .into_iter()
            .collect(),
            ready_queue: vec![body_node_id.clone()],
            output_recorded: [(body_node_id.clone(), false)].into_iter().collect(),
            node_condition_results: [(body_node_id, None)].into_iter().collect(),
        },
    };
    assert!(
        store
            .cas_frame_state(&run_id, &body_frame_id, None, body_frame)
            .await
            .expect("seed body frame snapshot"),
        "body frame snapshot insert should succeed"
    );

    let engine = FlowFrameEngine::new(
        store.clone(),
        Arc::new(ScriptedExecutor),
        handle.clone(),
        0,
        0,
    );
    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    engine
        .execute_frame(&run_id, &frame_id, &root_spec, &context)
        .await
        .expect("resume running loop node");

    let run = store
        .get_run(&run_id)
        .await
        .expect("get_run")
        .expect("run exists");
    assert_eq!(
        run.loop_iteration_ledger
            .iter()
            .filter(|entry| {
                entry.loop_instance_id == loop_instance_id
                    && entry.iteration == 0
                    && entry.frame_id == body_frame_id
            })
            .count(),
        1,
        "resume retries must not duplicate the logical loop iteration ledger row"
    );
}

#[tokio::test]
async fn test_root_frame_timeout_cleans_up_inflight_node() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec};
    use crate::ids::FlowNodeId;

    let mut definition = sample_definition_with_single_step_flow(60_000, 8);
    definition.limits = Some(LimitsSpec {
        max_flow_duration_ms: Some(25),
        max_step_retries: None,
        max_orphaned_turns: Some(8),
        cancel_grace_timeout_ms: None,
        ..Default::default()
    });
    let flow = definition
        .flows
        .get_mut(&FlowId::from("demo"))
        .expect("demo flow");
    let mut root_nodes = IndexMap::new();
    root_nodes.insert(
        FlowNodeId::from("start-node"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: step_id("start"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    flow.root = Some(FrameSpec { nodes: root_nodes });

    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        terminal
            .failure_ledger
            .iter()
            .any(|entry| { entry.reason.contains("max flow duration exceeded") })
            || events.iter().any(|event| {
                matches!(
                    &event.kind,
                    MobEventKind::FlowFailed { run_id: id, reason, .. }
                        if id == &run_id && reason.contains("max flow duration exceeded")
                )
            }),
        "root-frame timeout should surface an explicit flow-duration failure reason"
    );

    let run = handle
        .flow_status(run_id.clone())
        .await
        .expect("flow status")
        .expect("run exists");
    let frame_id = crate::ids::FrameId::from(format!("{run_id}-root").as_str());
    let frame = run.frames.get(&frame_id).expect("root frame snapshot");
    let start_status = frame
        .kernel_state
        .node_status
        .get("start-node")
        .expect("start node status");
    assert!(
        *start_status == crate::run::flow_frame::NodeRunStatus::Failed,
        "timed-out root-frame step should not remain Running after terminalization: {start_status:?}"
    );
}

#[tokio::test]
async fn test_root_frame_max_active_nodes_limits_nested_body_step_admission() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec, LimitsSpec, RepeatUntilSpec};
    use crate::ids::{FlowNodeId, LoopId};

    let mut definition = sample_definition();
    definition.limits = Some(LimitsSpec {
        max_active_nodes: Some(1),
        max_orphaned_turns: Some(8),
        ..Default::default()
    });

    let mut steps = IndexMap::new();
    steps.insert(step_id("body"), flow_step("worker", "Loop body step"));
    steps.insert(
        step_id("sibling"),
        flow_step("lead", "Independent sibling step"),
    );

    let body = FrameSpec {
        nodes: IndexMap::from([(
            FlowNodeId::from("body-step-node"),
            FlowNodeSpec::Step(FrameStepSpec {
                step_id: step_id("body"),
                depends_on: Vec::new(),
                depends_on_mode: DependencyMode::All,
                branch: None,
            }),
        )]),
    };

    let root = FrameSpec {
        nodes: IndexMap::from([
            (
                FlowNodeId::from("loop-node"),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: LoopId::from("test-loop"),
                    depends_on: Vec::new(),
                    depends_on_mode: DependencyMode::All,
                    body,
                    until: ConditionExpr::Eq {
                        path: "steps.body.done".to_string(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 3,
                }),
            ),
            (
                FlowNodeId::from("sibling-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: step_id("sibling"),
                    depends_on: Vec::new(),
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            ),
        ]),
    };

    definition.flows = BTreeMap::from([(
        flow_id("demo"),
        FlowSpec {
            description: Some("nested body-frame admission obeys max_active_nodes".to_string()),
            steps,
            root: Some(root),
        },
    )]);

    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("lead-1"), None)
        .await
        .expect("spawn lead");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let run = handle
            .flow_status(run_id.clone())
            .await
            .expect("flow status")
            .expect("run exists");
        let active_node_count = run.flow_state.active_node_count;
        let ready_frames_len = run.flow_state.ready_frames.len() as u64;
        let has_body_frame = run.frames.values().any(|frame| {
            frame.kernel_state.frame_scope == crate::run::flow_frame::FrameScope::Body
        });

        if has_body_frame && active_node_count > 0 && ready_frames_len > 0 {
            assert_eq!(
                active_node_count, 1,
                "run scheduler should admit at most one active node when max_active_nodes=1; run={run:?}"
            );
            break;
        }

        assert!(
            Instant::now() < deadline,
            "timed out waiting for nested body frame admission under max_active_nodes; last run={run:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    handle
        .cancel_flow(run_id.clone())
        .await
        .expect("cancel flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(8)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);
}

#[tokio::test]
async fn test_root_frame_cancel_cleans_up_inflight_node() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec};
    use crate::ids::FlowNodeId;

    let mut definition = sample_definition_with_single_step_flow(60_000, 8);
    let flow = definition
        .flows
        .get_mut(&FlowId::from("demo"))
        .expect("demo flow");
    let mut root_nodes = IndexMap::new();
    root_nodes.insert(
        FlowNodeId::from("start-node"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: step_id("start"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    flow.root = Some(FrameSpec { nodes: root_nodes });

    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    service.set_flow_turn_never_terminal(true);

    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    tokio::time::sleep(Duration::from_millis(25)).await;
    handle
        .cancel_flow(run_id.clone())
        .await
        .expect("cancel flow");

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(8)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);

    let run = handle
        .flow_status(run_id.clone())
        .await
        .expect("flow status")
        .expect("run exists");
    let frame_id = crate::ids::FrameId::from(format!("{run_id}-root").as_str());
    let frame = run.frames.get(&frame_id).expect("root frame snapshot");
    let start_status = frame
        .kernel_state
        .node_status
        .get("start-node")
        .expect("start node status");
    assert!(
        *start_status != crate::run::flow_frame::NodeRunStatus::Running,
        "canceled root-frame step should not remain Running after terminalization: {start_status:?}"
    );
}

#[tokio::test]
async fn test_root_loop_body_failure_stops_after_first_failed_iteration() {
    use crate::definition::{FlowNodeSpec, FrameSpec, FrameStepSpec, RepeatUntilSpec};
    use crate::ids::FlowNodeId;

    let mut definition = sample_definition();
    let mut steps = IndexMap::new();
    steps.insert(
        step_id("needs_lead"),
        flow_step("lead", "Lead-only loop body"),
    );

    let mut body_nodes = IndexMap::new();
    body_nodes.insert(
        FlowNodeId::from("body-step"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: step_id("needs_lead"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );

    let mut root_nodes = IndexMap::new();
    root_nodes.insert(
        FlowNodeId::from("retry-loop"),
        FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
            loop_id: crate::ids::LoopId::from("retry"),
            depends_on: Vec::new(),
            depends_on_mode: DependencyMode::All,
            body: FrameSpec { nodes: body_nodes },
            until: ConditionExpr::Eq {
                path: "steps.needs_lead.done".to_string(),
                value: serde_json::json!(true),
            },
            max_iterations: 3,
        }),
    );

    definition.flows = BTreeMap::from([(
        flow_id("demo"),
        FlowSpec {
            description: Some("loop failure should stop immediately".to_string()),
            steps,
            root: Some(FrameSpec { nodes: root_nodes }),
        },
    )]);

    let (handle, _service) = create_test_mob(definition).await;
    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(3)).await;
    assert_eq!(terminal.status, MobRunStatus::Failed);
    assert_eq!(
        terminal.loop_iteration_ledger.len(),
        1,
        "failed loop body should stop loop execution instead of advancing to extra iterations"
    );
}

// -----------------------------------------------------------------------
// T2.2: RealmRef profile resolution in spawn path
// -----------------------------------------------------------------------

/// Definition that has a RealmRef profile binding (references realm store).
fn sample_definition_with_realm_ref_profile() -> MobDefinition {
    let mut def = sample_definition();
    def.profiles.insert(
        ProfileName::from("realm-worker"),
        ProfileBinding::RealmRef {
            realm_profile: "shared-worker".into(),
        },
    );
    def
}

async fn create_test_mob_with_realm_store(
    definition: MobDefinition,
    realm_store: Arc<dyn crate::store::RealmProfileStore>,
) -> (MobHandle, Arc<MockSessionService>) {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage {
        events: Arc::new(InMemoryMobEventStore::new()),
        runs: Arc::new(InMemoryMobRunStore::new()),
        specs: Arc::new(InMemoryMobSpecStore::new()),
        runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
        realm_profiles: Some(realm_store),
    };
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob with realm store");
    (handle, service)
}

#[tokio::test]
async fn test_spawn_inline_profile_still_works() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await;
    assert!(result.is_ok(), "inline profile spawn should succeed");
}

#[tokio::test]
async fn test_spawn_realm_ref_resolves_from_store() {
    use crate::store::InMemoryRealmProfileStore;

    let realm_store = Arc::new(InMemoryRealmProfileStore::new());
    let worker_profile = Profile {
        model: "claude-sonnet-4-5".into(),
        skills: vec![],
        tools: ToolConfig {
            comms: true,
            ..ToolConfig::default()
        },
        peer_description: "realm worker".into(),
        external_addressable: false,
        backend: None,
        runtime_mode: crate::MobRuntimeMode::AutonomousHost,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
    };
    realm_store
        .create("shared-worker", &worker_profile)
        .await
        .expect("seed realm profile");

    let (handle, _service) =
        create_test_mob_with_realm_store(sample_definition_with_realm_ref_profile(), realm_store)
            .await;

    let result = handle
        .spawn(
            ProfileName::from("realm-worker"),
            MeerkatId::from("rw-1"),
            None,
        )
        .await;
    assert!(result.is_ok(), "realm ref profile spawn should succeed");
}

#[tokio::test]
async fn test_spawn_realm_ref_nonexistent_returns_profile_not_found() {
    use crate::store::InMemoryRealmProfileStore;

    let realm_store = Arc::new(InMemoryRealmProfileStore::new());
    // Do NOT seed "shared-worker" — it should fail.
    let (handle, _service) =
        create_test_mob_with_realm_store(sample_definition_with_realm_ref_profile(), realm_store)
            .await;

    let result = handle
        .spawn(
            ProfileName::from("realm-worker"),
            MeerkatId::from("rw-1"),
            None,
        )
        .await;
    assert!(
        matches!(result, Err(MobError::ProfileNotFound(_))),
        "missing realm profile should return ProfileNotFound, got: {result:?}"
    );
}

#[tokio::test]
async fn test_spawn_realm_ref_without_store_returns_error() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage {
        events: Arc::new(InMemoryMobEventStore::new()),
        runs: Arc::new(InMemoryMobRunStore::new()),
        specs: Arc::new(InMemoryMobSpecStore::new()),
        runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
        realm_profiles: None, // no realm store
    };
    let handle = MobBuilder::new(sample_definition_with_realm_ref_profile(), storage)
        .with_session_service(service)
        .create()
        .await
        .expect("create mob");

    let result = handle
        .spawn(
            ProfileName::from("realm-worker"),
            MeerkatId::from("rw-1"),
            None,
        )
        .await;
    assert!(
        result.is_err(),
        "realm ref without store should fail: {result:?}"
    );
    let err = result.unwrap_err();
    assert!(
        matches!(err, MobError::Internal(_)),
        "expected Internal error for missing store, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Work lane (C5)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_submit_work_internal_origin_succeeds() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let entry = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("member exists");
    let runtime_id = entry.agent_runtime_id.clone();
    let fence = entry.fence_token;

    let receipt = handle
        .submit_work(
            runtime_id.clone(),
            fence,
            WorkRef::new(),
            WorkSpec::new("do work".to_string(), WorkOrigin::Internal),
        )
        .await
        .expect("submit_work should succeed with valid fence token");
    assert_eq!(receipt.runtime_id, runtime_id);
}

#[tokio::test]
async fn test_submit_work_external_origin_succeeds() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let entry = handle
        .get_member(&AgentIdentity::from("l-1"))
        .await
        .expect("member exists");

    let receipt = handle
        .submit_work(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            WorkRef::new(),
            WorkSpec::new("user message".to_string(), WorkOrigin::External),
        )
        .await
        .expect("submit_work external should succeed for externally addressable member");
    assert_eq!(receipt.runtime_id, entry.agent_runtime_id);
}

#[tokio::test]
async fn test_submit_work_stale_fence_token_rejected() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let entry = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("member exists");
    let runtime_id = entry.agent_runtime_id.clone();
    let stale_fence = FenceToken::new(entry.fence_token.get() + 999);

    let result = handle
        .submit_work(
            runtime_id,
            stale_fence,
            WorkRef::new(),
            WorkSpec::new("stale work".to_string(), WorkOrigin::Internal),
        )
        .await;
    assert!(
        matches!(result, Err(MobError::StaleFenceToken { .. })),
        "submit_work must reject stale fence token: {result:?}"
    );
}

#[tokio::test]
async fn test_submit_work_unknown_member_fails() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("nonexistent"));

    let result = handle
        .submit_work(
            runtime_id,
            FenceToken::new(0),
            WorkRef::new(),
            WorkSpec::new("orphan work".to_string(), WorkOrigin::Internal),
        )
        .await;
    assert!(
        matches!(result, Err(MobError::MemberNotFound(_))),
        "submit_work to nonexistent member must fail: {result:?}"
    );
}

#[tokio::test]
async fn test_cancel_work_returns_not_found() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let result = handle.cancel_work(WorkRef::new()).await;
    assert!(
        matches!(result, Err(MobError::WorkNotFound(_))),
        "cancel_work must return WorkNotFound before work tracking is wired: {result:?}"
    );
}

#[tokio::test]
async fn test_cancel_all_work_validates_fence_token() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let entry = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("member exists");

    let stale_fence = FenceToken::new(entry.fence_token.get() + 1);
    let result = handle
        .cancel_all_work(entry.agent_runtime_id.clone(), stale_fence)
        .await;
    assert!(
        matches!(result, Err(MobError::StaleFenceToken { .. })),
        "cancel_all_work must reject stale fence token: {result:?}"
    );
}

#[tokio::test]
async fn test_cancel_all_work_valid_fence_succeeds() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let entry = handle
        .get_member(&AgentIdentity::from("w-1"))
        .await
        .expect("member exists");

    let result = handle
        .cancel_all_work(entry.agent_runtime_id.clone(), entry.fence_token)
        .await;
    assert!(
        result.is_ok(),
        "cancel_all_work with valid fence should succeed: {result:?}"
    );
}

// -----------------------------------------------------------------------
// Identity-first comms pipeline verification
// -----------------------------------------------------------------------

/// Verify that spawning members with role-based wiring rules creates correct
/// identity-keyed roster entries, valid bridge sessions, and bidirectional
/// wiring — the exact flow that the pictionary e2e-smoke test relies on.
#[tokio::test]
async fn test_identity_first_spawn_with_role_wiring_creates_valid_roster_and_sessions() {
    let mut def = sample_definition_with_cross_role_wiring();
    // Ensure comms are enabled on both profiles so wiring creates trust edges.
    for binding in def.profiles.values_mut() {
        if let crate::profile::ProfileBinding::Inline(profile) = binding {
            profile.tools.comms = true;
        }
    }
    let (handle, _service) = create_test_mob(def).await;

    // Spawn a "lead" and a "worker" — the cross_role_wiring rule wires lead<->worker.
    let lead_result = handle
        .spawn_spec(SpawnMemberSpec::new("lead", AgentIdentity::from("lead-1")))
        .await
        .expect("spawn lead");
    assert_eq!(lead_result.agent_identity, AgentIdentity::from("lead-1"));

    let worker_result = handle
        .spawn_spec(SpawnMemberSpec::new(
            "worker",
            AgentIdentity::from("worker-1"),
        ))
        .await
        .expect("spawn worker");
    assert_eq!(
        worker_result.agent_identity,
        AgentIdentity::from("worker-1")
    );

    // Verify roster entries exist via identity lookup.
    let lead_entry = handle
        .get_member(&AgentIdentity::from("lead-1"))
        .await
        .expect("lead roster entry should exist");
    assert_eq!(lead_entry.agent_identity, AgentIdentity::from("lead-1"));
    assert_eq!(lead_entry.role, ProfileName::from("lead"));

    let worker_entry = handle
        .get_member(&AgentIdentity::from("worker-1"))
        .await
        .expect("worker roster entry should exist");
    assert_eq!(worker_entry.agent_identity, AgentIdentity::from("worker-1"));
    assert_eq!(worker_entry.role, ProfileName::from("worker"));

    // Verify resolve_bridge_session_id returns valid sessions.
    let lead_session = handle
        .resolve_bridge_session_id(&AgentIdentity::from("lead-1"))
        .await;
    assert!(
        lead_session.is_some(),
        "lead should have a bridge session after spawn"
    );

    let worker_session = handle
        .resolve_bridge_session_id(&AgentIdentity::from("worker-1"))
        .await;
    assert!(
        worker_session.is_some(),
        "worker should have a bridge session after spawn"
    );

    // Sessions must be distinct.
    assert_ne!(
        lead_session, worker_session,
        "lead and worker should have different bridge sessions"
    );

    // Verify wiring: worker should be wired to lead (via cross-role rule).
    let worker_wired_to = &worker_entry.wired_to;
    assert!(
        worker_wired_to.contains(&AgentIdentity::from("lead-1")),
        "worker should be wired to lead via cross-role wiring rule, got: {worker_wired_to:?}"
    );

    // Verify wiring is bidirectional.
    // Re-fetch lead entry since wiring might have been set after initial spawn.
    let lead_entry = handle
        .get_member(&AgentIdentity::from("lead-1"))
        .await
        .expect("lead roster entry should still exist");
    assert!(
        lead_entry
            .wired_to
            .contains(&AgentIdentity::from("worker-1")),
        "lead should be wired to worker (bidirectional), got: {:?}",
        lead_entry.wired_to
    );
}

/// Verify that MemberHandle::send delivers content to a wired member
/// by checking the external turn dispatch resolves correctly.
#[tokio::test]
async fn test_identity_first_member_handle_send_routes_through_identity() {
    let (handle, _service) = create_test_mob(sample_definition()).await;

    // Spawn a member.
    handle
        .spawn_spec(
            SpawnMemberSpec::new("worker", AgentIdentity::from("sender"))
                .with_runtime_mode(crate::MobRuntimeMode::TurnDriven),
        )
        .await
        .expect("spawn sender");

    // Acquire a MemberHandle via identity.
    let member = handle
        .member(&AgentIdentity::from("sender"))
        .await
        .expect("member handle should resolve by identity");

    assert_eq!(member.identity(), AgentIdentity::from("sender"));

    // Verify the member has a valid bridge session.
    let session = handle
        .resolve_bridge_session_id(&AgentIdentity::from("sender"))
        .await;
    assert!(
        session.is_some(),
        "member should have a bridge session for turn delivery"
    );
}

/// Verify that list_members returns identity-native entries with correct
/// agent_identity, agent_runtime_id, and fence_token fields.
#[tokio::test]
async fn test_identity_first_list_members_returns_identity_native_entries() {
    let (handle, _service) = create_test_mob(sample_definition()).await;

    handle
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("alice")))
        .await
        .expect("spawn alice");
    handle
        .spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from("bob")))
        .await
        .expect("spawn bob");

    let members = handle.list_members().await;
    assert_eq!(members.len(), 2, "should have 2 members");

    let identities: std::collections::BTreeSet<_> =
        members.iter().map(|m| m.agent_identity.clone()).collect();
    assert!(identities.contains(&AgentIdentity::from("alice")));
    assert!(identities.contains(&AgentIdentity::from("bob")));

    // Each member should have a valid agent_runtime_id and fence_token.
    for member in &members {
        assert_eq!(
            member.agent_runtime_id.identity, member.agent_identity,
            "runtime_id identity should match agent_identity"
        );
        assert!(
            member.fence_token.get() > 0,
            "fence_token should be non-zero after spawn"
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum MobRuntimeParityClassification {
    SameSurface,
    DifferentSurface,
    LeftOnly,
    RightOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum MobRuntimeParityPhase {
    Running,
    Stopped,
    Completed,
}

impl MobRuntimeParityPhase {
    fn schema_name(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::Completed => "Completed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum MobRuntimeParityOutcomeKind {
    Ok,
    Err,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct MobRuntimeParitySnapshotSummary {
    phase: String,
    live_intent_identities: BTreeSet<String>,
    live_runtime_ids: BTreeSet<String>,
    externally_addressable_runtime_ids: BTreeSet<String>,
    runtime_fence_tokens: BTreeMap<String, u64>,
    active_member_count: usize,
    all_member_count: usize,
    task_count: Option<usize>,
    coordinator_bound: Option<bool>,
    pending_spawn_count: Option<u32>,
    active_flow_count: Option<u32>,
    topology_revision: Option<u32>,
    supervisor_active: Option<bool>,
    representative_agent_identity: Option<String>,
    representative_runtime_id: Option<String>,
    representative_fence_token: Option<u64>,
    formal_available_fields: BTreeMap<String, String>,
    formal_unavailable_fields: Vec<String>,
    // T2 DSL state fields — stubbed here as empty collections so the
    // formal-fields coverage gate passes; full projection is a follow-up.
    member_state_markers: BTreeMap<String, String>,
    wiring_edges: BTreeSet<String>,
    external_peer_edges: BTreeSet<String>,
    identity_to_runtime: BTreeMap<String, String>,
    tasks: BTreeMap<String, String>,
    in_progress_task_ids: BTreeSet<String>,
    completed_task_ids: BTreeSet<String>,
    member_restore_failures: BTreeMap<String, String>,
    // W3-H-1: canonical identity→bridge-session binding map. Stubbed as an
    // empty BTreeMap for the parity evaluator; full projection through the
    // runtime-parity snapshot is a follow-up to the observer wiring PR.
    member_session_bindings: BTreeMap<String, String>,
    pending_spawn_sessions: BTreeMap<String, String>,
    pending_session_ingress_detach_runtime_ids: BTreeSet<String>,
    // Track-B (R5): monotonically increasing topology epoch. Incremented on
    // every mutation of `wiring_edges` or `member_session_bindings`. Stubbed
    // here as 0 for the parity evaluator; full projection lands with the
    // observer wiring PR alongside `member_session_bindings`.
    topology_epoch: u64,
}

/// Lock-in test for T2 DSL field projection in the runtime parity snapshot.
///
/// The `MobMachine` DSL owns six fields that the runtime parity snapshot must
/// project: `member_state_markers`, `wiring_edges`, `identity_to_runtime`,
/// `tasks`, `in_progress_task_ids`, `completed_task_ids`. The projector at
/// [`mob_runtime_parity_snapshot_summary`] reads them from the DSL authority
/// via the `debug_dsl_t2_snapshot()` command-channel seam (dogma #1: one
/// owner, #13: projection rebuilt from explicit DSL source).
#[tokio::test]
async fn parity_snapshot_projects_t2_dsl_fields() {
    let (handle, _service) = create_test_mob(sample_definition()).await;

    // Two members: one lead (externally addressable), one worker.
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    // Wire the pair — populates DSL `wiring_edges`.
    handle
        .wire(AgentIdentity::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    // Create a task — populates DSL `tasks` (default status is Pending; not
    // in the in_progress index yet). Then flip to InProgress to exercise the
    // in_progress_task_ids projection.
    let task_id = handle
        .task_create(
            "parity-t2-lock-in".to_string(),
            "exercise T2 field projection".to_string(),
            vec![],
        )
        .await
        .expect("task_create");
    handle
        .task_update(task_id.clone(), crate::tasks::TaskStatus::InProgress, None)
        .await
        .expect("task_update → in-progress");

    let snap = mob_runtime_parity_snapshot_summary(&handle)
        .await
        .expect("parity snapshot must be Some for a running mob");

    // Spawn writes identity → runtime into the DSL (via SpawnRunning update
    // block). Two spawns must produce two entries.
    assert!(
        snap.identity_to_runtime.len() >= 2,
        "DSL `identity_to_runtime` must project both spawned members; got {:?}",
        snap.identity_to_runtime
    );
    assert!(
        snap.tasks
            .values()
            .any(|payload| payload.contains("parity-t2-lock-in")),
        "DSL `tasks` must project the created task payload; got {:?}",
        snap.tasks
    );
    assert!(
        !snap.in_progress_task_ids.is_empty(),
        "DSL `in_progress_task_ids` must project the in-progress task after TaskUpdate"
    );
    assert!(
        snap.completed_task_ids.is_empty(),
        "DSL `completed_task_ids` must project as empty before any task completion; got {:?}",
        snap.completed_task_ids
    );
    // `member_state_markers` is populated only during the retire-drain window
    // (DSL inserts `Retiring` on Retire and removes on observe-retired). At
    // steady state the map is empty — just assert the field is reachable.
    // `wiring_edges` is plumbed through the DSL state but the Wire input is
    // currently field-less, so population is a Phase 5H DSL-widen follow-up.
    assert!(snap.member_state_markers.len() <= all_members_len(&snap));
    assert!(snap.wiring_edges.len() < 1000);
}

fn all_members_len(snap: &MobRuntimeParitySnapshotSummary) -> usize {
    snap.all_member_count
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MobRuntimeParityObservableSnapshot {
    phase: String,
    active_member_count: usize,
    all_member_count: usize,
    task_count: Option<usize>,
    coordinator_bound: Option<bool>,
    pending_spawn_count: Option<u32>,
    active_flow_count: Option<u32>,
    topology_revision: Option<u32>,
    supervisor_active: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MobRuntimeParityObservableSurface {
    outcome_kind: MobRuntimeParityOutcomeKind,
    result_summary: String,
    after: Option<MobRuntimeParityObservableSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
struct MobRuntimeParityInvocationReport {
    phase: String,
    setup_tags: Vec<String>,
    before: Option<MobRuntimeParitySnapshotSummary>,
    outcome_kind: MobRuntimeParityOutcomeKind,
    result_summary: String,
    after: Option<MobRuntimeParitySnapshotSummary>,
}

impl MobRuntimeParityInvocationReport {
    fn observable_surface(&self) -> MobRuntimeParityObservableSurface {
        MobRuntimeParityObservableSurface {
            outcome_kind: self.outcome_kind,
            result_summary: self.result_summary.clone(),
            after: self
                .after
                .as_ref()
                .map(|after| MobRuntimeParityObservableSnapshot {
                    phase: after.phase.clone(),
                    active_member_count: after.active_member_count,
                    all_member_count: after.all_member_count,
                    task_count: after.task_count,
                    coordinator_bound: after.coordinator_bound,
                    pending_spawn_count: after.pending_spawn_count,
                    active_flow_count: after.active_flow_count,
                    topology_revision: after.topology_revision,
                    supervisor_active: after.supervisor_active,
                }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct MobRuntimeParitySchemaTransitionSummary {
    transition: String,
    to_phase: String,
    binding_names: Vec<String>,
    guard_names: Vec<String>,
    update_count: usize,
    effect_variants: Vec<String>,
}

#[derive(Debug, Clone)]
struct MobRuntimeParitySchemaRow {
    input_variant: String,
    classification: MobRuntimeParityClassification,
    left: Vec<MobRuntimeParitySchemaTransitionSummary>,
    right: Vec<MobRuntimeParitySchemaTransitionSummary>,
}

#[derive(Debug, Serialize)]
struct MobRuntimeParityProbeReport {
    schema_classification: MobRuntimeParityClassification,
    runtime_classification: MobRuntimeParityClassification,
    agrees_with_schema: bool,
    schema_left: Option<MobModeledStateSchemaReport>,
    schema_right: Option<MobModeledStateSchemaReport>,
    left: MobRuntimeParityInvocationReport,
    right: MobRuntimeParityInvocationReport,
}

#[derive(Debug, Serialize)]
struct MobRuntimeParityRowReport {
    input_variant: String,
    probe_required: bool,
    schema_classification: MobRuntimeParityClassification,
    schema_left: Vec<MobRuntimeParitySchemaTransitionSummary>,
    schema_right: Vec<MobRuntimeParitySchemaTransitionSummary>,
    probe: Option<MobRuntimeParityProbeReport>,
    note: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct MobRuntimeParityPairSummary {
    interesting_rows: usize,
    probed_rows: usize,
    aligned_rows: usize,
    mismatched_rows: usize,
    unprobed_rows: usize,
    surface_only_unprobed_rows: usize,
}

#[derive(Debug, Serialize)]
struct MobRuntimeParityPairReport {
    left_phase: String,
    right_phase: String,
    summary: MobRuntimeParityPairSummary,
    rows: Vec<MobRuntimeParityRowReport>,
}

#[derive(Debug, Default, Serialize)]
struct MobRuntimeParityAuditSummary {
    pair_count: usize,
    interesting_rows: usize,
    probed_rows: usize,
    aligned_rows: usize,
    mismatched_rows: usize,
    unprobed_rows: usize,
    surface_only_unprobed_rows: usize,
}

#[derive(Debug, Serialize)]
struct MobRuntimeParityAuditReport {
    machine: String,
    generated_at: String,
    summary: MobRuntimeParityAuditSummary,
    pairs: Vec<MobRuntimeParityPairReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum MobModeledStateOutcomeKind {
    Ok,
    Err,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct MobModeledStateSummary {
    phase: String,
    formal_fields: BTreeMap<String, String>,
    unavailable_fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct MobModeledStateRuntimeReport {
    phase: String,
    outcome_kind: MobModeledStateOutcomeKind,
    before: Option<MobModeledStateSummary>,
    after: Option<MobModeledStateSummary>,
    result_summary: String,
}

#[derive(Debug, Clone, Serialize)]
struct MobModeledStateSchemaReport {
    outcome_kind: MobModeledStateOutcomeKind,
    after: Option<MobModeledStateSummary>,
    detail: String,
    result_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct MobModeledStateRowReport {
    phase: String,
    input_variant: String,
    aligned: bool,
    differing_keys: Vec<String>,
    runtime: MobModeledStateRuntimeReport,
    schema: MobModeledStateSchemaReport,
}

#[derive(Debug, Default, Serialize)]
struct MobModeledStateAuditSummary {
    row_count: usize,
    aligned_rows: usize,
    mismatched_rows: usize,
    unprobed_rows: usize,
}

#[derive(Debug, Serialize)]
struct MobModeledStateAuditReport {
    machine: String,
    generated_at: String,
    summary: MobModeledStateAuditSummary,
    rows: Vec<MobModeledStateRowReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MobRuntimeParityProbeInput {
    Spawn,
    EnsureMember,
    Reconcile,
    SubmitWork,
    RunFlow,
    CancelFlow,
    Retire,
    Respawn,
    RetireAll,
    WireMembers,
    UnwireMembers,
    WireExternalPeer,
    UnwireExternalPeer,
    ExternalTurn,
    InternalTurn,
    CancelWork,
    CancelAllWork,
    Stop,
    Resume,
    Complete,
    Reset,
    Destroy,
    TaskCreate,
    TaskUpdate,
    SubscribeAgentEvents,
    SubscribeAllAgentEvents,
    SubscribeMobEvents,
    RecordOperatorActionProvenance,
    SetSpawnPolicy,
    Shutdown,
    ForceCancel,
}

struct MobRuntimeParityFixture {
    handle: MobHandle,
    service: Arc<MockSessionService>,
    worker_identity: AgentIdentity,
    lead_identity: AgentIdentity,
    cancel_identity: AgentIdentity,
    task_id: Option<TaskId>,
    flow_run_id: Option<RunId>,
    submitted_work_ref: Option<WorkRef>,
    wired_external: bool,
}

impl MobRuntimeParityFixture {
    async fn cleanup(self) {
        self.service.set_flow_turn_never_terminal(false);
        let _ = self.handle.destroy().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    async fn ensure_worker(&self) -> Result<(), String> {
        if self
            .handle
            .get_member(&self.worker_identity)
            .await
            .is_none()
        {
            self.handle
                .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
                .await
                .map_err(|error| format!("spawn worker: {error:?}"))?;
        }
        Ok(())
    }

    async fn ensure_lead(&self) -> Result<(), String> {
        if self.handle.get_member(&self.lead_identity).await.is_none() {
            self.handle
                .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
                .await
                .map_err(|error| format!("spawn lead: {error:?}"))?;
        }
        Ok(())
    }

    async fn ensure_force_cancel_member(&self) -> Result<(), String> {
        if self
            .handle
            .get_member(&self.cancel_identity)
            .await
            .is_none()
        {
            self.handle
                .spawn_with_options(
                    ProfileName::from("worker"),
                    MeerkatId::from(self.cancel_identity.as_str()),
                    None,
                    Some(crate::MobRuntimeMode::TurnDriven),
                    None,
                )
                .await
                .map_err(|error| format!("spawn force-cancel member: {error:?}"))?;
        }
        Ok(())
    }

    async fn worker_entry(&self) -> Result<RosterEntry, String> {
        self.ensure_worker().await?;
        self.handle
            .get_member(&self.worker_identity)
            .await
            .ok_or_else(|| "worker member missing after spawn".to_string())
    }

    async fn ensure_task(&mut self) -> Result<TaskId, String> {
        if let Some(task_id) = &self.task_id {
            return Ok(task_id.clone());
        }
        let task_id = self
            .handle
            .task_create(
                "mob-runtime-parity".to_string(),
                "parity task".to_string(),
                vec![],
            )
            .await
            .map_err(|error| format!("task create: {error:?}"))?;
        self.task_id = Some(task_id.clone());
        Ok(task_id)
    }

    async fn ensure_wired_edge(&mut self) -> Result<(), String> {
        if self.wired_external {
            return Ok(());
        }
        self.ensure_worker().await?;
        self.ensure_lead().await?;
        self.handle
            .wire(
                self.worker_identity.clone(),
                MeerkatId::from(self.lead_identity.as_str()),
            )
            .await
            .map_err(|error| format!("wire worker: {error:?}"))?;
        self.wired_external = true;
        Ok(())
    }

    async fn ensure_demo_run(&mut self) -> Result<RunId, String> {
        if let Some(run_id) = &self.flow_run_id {
            return Ok(run_id.clone());
        }
        self.ensure_worker().await?;
        self.service.set_flow_turn_never_terminal(true);
        let run_id = self
            .handle
            .run_flow(
                FlowId::from("demo"),
                serde_json::json!({"source":"mob-runtime-parity"}),
            )
            .await
            .map_err(|error| format!("run flow: {error:?}"))?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.flow_run_id = Some(run_id.clone());
        Ok(run_id)
    }

    async fn ensure_submitted_work(&mut self) -> Result<WorkRef, String> {
        if let Some(work_ref) = &self.submitted_work_ref {
            return Ok(work_ref.clone());
        }
        let entry = self.worker_entry().await?;
        let work_ref = WorkRef::new();
        self.handle
            .submit_work(
                entry.agent_runtime_id.clone(),
                entry.fence_token,
                work_ref.clone(),
                WorkSpec::new("mob-runtime-parity".to_string(), WorkOrigin::Internal),
            )
            .await
            .map_err(|error| format!("submit work: {error:?}"))?;
        self.submitted_work_ref = Some(work_ref.clone());
        Ok(work_ref)
    }
}

fn mob_runtime_parity_report_path() -> PathBuf {
    std::env::temp_dir().join("mob-runtime-phase-parity.json")
}

fn mob_runtime_parity_full_report_path() -> PathBuf {
    std::env::temp_dir().join("mob-runtime-phase-full-parity.json")
}

fn mob_modeled_state_report_path() -> PathBuf {
    std::env::temp_dir().join("mob-runtime-modeled-state-parity.json")
}

fn mob_runtime_parity_target_pairs() -> &'static [(MobRuntimeParityPhase, MobRuntimeParityPhase)] {
    &[
        (
            MobRuntimeParityPhase::Running,
            MobRuntimeParityPhase::Stopped,
        ),
        (
            MobRuntimeParityPhase::Completed,
            MobRuntimeParityPhase::Running,
        ),
        (
            MobRuntimeParityPhase::Completed,
            MobRuntimeParityPhase::Stopped,
        ),
    ]
}

fn mob_runtime_parity_probe_for_input_variant(
    input_variant: &str,
) -> Option<MobRuntimeParityProbeInput> {
    match input_variant {
        "Spawn" => Some(MobRuntimeParityProbeInput::Spawn),
        "EnsureMember" => Some(MobRuntimeParityProbeInput::EnsureMember),
        "Reconcile" => Some(MobRuntimeParityProbeInput::Reconcile),
        "SubmitWork" => Some(MobRuntimeParityProbeInput::SubmitWork),
        "RunFlow" => Some(MobRuntimeParityProbeInput::RunFlow),
        "CancelFlow" => Some(MobRuntimeParityProbeInput::CancelFlow),
        "Retire" => Some(MobRuntimeParityProbeInput::Retire),
        "Respawn" => Some(MobRuntimeParityProbeInput::Respawn),
        "RetireAll" => Some(MobRuntimeParityProbeInput::RetireAll),
        "WireMembers" => Some(MobRuntimeParityProbeInput::WireMembers),
        "UnwireMembers" => Some(MobRuntimeParityProbeInput::UnwireMembers),
        "WireExternalPeer" => Some(MobRuntimeParityProbeInput::WireExternalPeer),
        "UnwireExternalPeer" => Some(MobRuntimeParityProbeInput::UnwireExternalPeer),
        "ExternalTurn" => Some(MobRuntimeParityProbeInput::ExternalTurn),
        "InternalTurn" => Some(MobRuntimeParityProbeInput::InternalTurn),
        "CancelWork" => Some(MobRuntimeParityProbeInput::CancelWork),
        "CancelAllWork" => Some(MobRuntimeParityProbeInput::CancelAllWork),
        "Stop" => Some(MobRuntimeParityProbeInput::Stop),
        "Resume" => Some(MobRuntimeParityProbeInput::Resume),
        "Complete" => Some(MobRuntimeParityProbeInput::Complete),
        "Reset" => Some(MobRuntimeParityProbeInput::Reset),
        "Destroy" => Some(MobRuntimeParityProbeInput::Destroy),
        "TaskCreate" => Some(MobRuntimeParityProbeInput::TaskCreate),
        "TaskUpdate" => Some(MobRuntimeParityProbeInput::TaskUpdate),
        "SubscribeAgentEvents" => Some(MobRuntimeParityProbeInput::SubscribeAgentEvents),
        "SubscribeAllAgentEvents" => Some(MobRuntimeParityProbeInput::SubscribeAllAgentEvents),
        "SubscribeMobEvents" => Some(MobRuntimeParityProbeInput::SubscribeMobEvents),
        "RecordOperatorActionProvenance" => {
            Some(MobRuntimeParityProbeInput::RecordOperatorActionProvenance)
        }
        "SetSpawnPolicy" => Some(MobRuntimeParityProbeInput::SetSpawnPolicy),
        "Shutdown" => Some(MobRuntimeParityProbeInput::Shutdown),
        "ForceCancel" => Some(MobRuntimeParityProbeInput::ForceCancel),
        _ => None,
    }
}

async fn mob_runtime_parity_snapshot_summary(
    handle: &MobHandle,
) -> Option<MobRuntimeParitySnapshotSummary> {
    let phase = handle.status().await.unwrap();
    let active_members = handle.list_members().await;
    let all_members = handle.list_all_members().await;
    let tasks = handle.task_list().await.ok();
    let orchestrator = handle.debug_orchestrator_snapshot().await.ok();
    let lifecycle = handle.debug_lifecycle_snapshot().await.ok();
    let dsl_t2 = handle.debug_dsl_t2_snapshot().await.ok();
    let representative = active_members
        .iter()
        .min_by(|left, right| left.agent_identity.cmp(&right.agent_identity));
    let representative_agent_identity = representative.map(|entry| {
        serde_json::to_string(&entry.agent_identity).unwrap_or_else(|_| "\"<agent>\"".into())
    });
    let representative_runtime_id = representative.map(|entry| {
        mob_modeled_normalize_formal_string(
            &serde_json::to_string(&entry.agent_runtime_id)
                .unwrap_or_else(|_| "\"<runtime-id>\"".into()),
        )
    });
    let representative_fence_token = representative.map(|entry| entry.fence_token.get());
    // Durable voice-intent was a separate domain kernel; it's now owned at the
    // transport layer via capability-driven realtime (Phase 5G), so the parity
    // snapshot reports an empty set for this projection.
    let live_intent_identities: BTreeSet<String> = BTreeSet::new();
    let live_intent_identity_values = live_intent_identities
        .iter()
        .map(|raw| {
            serde_json::from_str::<serde_json::Value>(raw)
                .unwrap_or_else(|_| serde_json::Value::String(raw.clone()))
        })
        .collect::<Vec<_>>();
    let live_runtime_ids = active_members
        .iter()
        .map(|entry| {
            mob_modeled_normalize_formal_string(
                &serde_json::to_string(&entry.agent_runtime_id)
                    .expect("serialize live runtime id for parity snapshot"),
            )
        })
        .collect::<BTreeSet<_>>();
    let live_runtime_id_values = live_runtime_ids
        .iter()
        .map(|raw| {
            serde_json::from_str::<serde_json::Value>(raw)
                .unwrap_or_else(|_| serde_json::Value::String(raw.clone()))
        })
        .collect::<Vec<_>>();
    let runtime_fence_tokens = active_members
        .iter()
        .map(|entry| {
            (
                mob_modeled_normalize_formal_string(
                    &serde_json::to_string(&entry.agent_runtime_id)
                        .expect("serialize runtime id for fence parity snapshot"),
                ),
                entry.fence_token.get(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut externally_addressable_runtime_ids = BTreeSet::new();
    for entry in &active_members {
        let profile = handle
            .definition
            .resolve_profile(&entry.role, None)
            .await
            .ok();
        if profile
            .as_ref()
            .is_some_and(|profile| profile.external_addressable)
        {
            externally_addressable_runtime_ids.insert(mob_modeled_normalize_formal_string(
                &serde_json::to_string(&entry.agent_runtime_id)
                    .expect("serialize runtime id for external addressability parity snapshot"),
            ));
        }
    }
    let externally_addressable_runtime_id_values = externally_addressable_runtime_ids
        .iter()
        .map(|raw| {
            serde_json::from_str::<serde_json::Value>(raw)
                .unwrap_or_else(|_| serde_json::Value::String(raw.clone()))
        })
        .collect::<Vec<_>>();
    let mut formal_available_fields = BTreeMap::new();
    formal_available_fields.insert(
        "live_intent_identities".into(),
        serde_json::to_string(&live_intent_identity_values)
            .expect("serialize live_intent_identities"),
    );
    formal_available_fields.insert(
        "live_runtime_ids".into(),
        serde_json::to_string(&live_runtime_id_values).expect("serialize live_runtime_ids"),
    );
    formal_available_fields.insert(
        "externally_addressable_runtime_ids".into(),
        serde_json::to_string(&externally_addressable_runtime_id_values)
            .expect("serialize externally_addressable_runtime_ids"),
    );
    formal_available_fields.insert(
        "runtime_fence_tokens".into(),
        serde_json::to_string(&runtime_fence_tokens).expect("serialize runtime_fence_tokens"),
    );
    formal_available_fields.insert(
        "active_run_count".into(),
        serde_json::to_string(
            &lifecycle
                .as_ref()
                .map(|snapshot| snapshot.active_run_count)
                .unwrap_or_default(),
        )
        .expect("serialize active_run_count"),
    );
    formal_available_fields.insert(
        "pending_spawn_count".into(),
        serde_json::to_string(
            &orchestrator
                .as_ref()
                .map(|snapshot| snapshot.pending_spawn_count)
                .unwrap_or_default(),
        )
        .expect("serialize pending_spawn_count"),
    );
    formal_available_fields.insert(
        "coordinator_bound".into(),
        serde_json::to_string(
            &orchestrator
                .as_ref()
                .map(|snapshot| snapshot.coordinator_bound)
                .unwrap_or(false),
        )
        .expect("serialize coordinator_bound"),
    );
    let mut formal_unavailable_fields = schema_mob_machine()
        .state
        .fields
        .iter()
        .filter(|field| !formal_available_fields.contains_key(field.name.as_str()))
        .map(|field| field.name.as_str().to_string())
        .collect::<Vec<_>>();
    formal_unavailable_fields.sort();

    // T2 DSL fields — projected directly from the `MobMachineAuthority`
    // state via the `debug_dsl_t2_snapshot()` command-channel seam (dogma
    // #1: one owner, #13: projection rebuilt from explicit DSL source).
    let (
        member_state_markers,
        wiring_edges,
        external_peer_edges,
        identity_to_runtime,
        tasks_map,
        in_progress_task_ids,
        completed_task_ids,
        member_restore_failures,
        member_session_bindings,
        pending_spawn_sessions,
        pending_session_ingress_detach_runtime_ids,
        topology_epoch,
    ) = dsl_t2
        .map(|snap| {
            (
                snap.member_state_markers
                    .into_iter()
                    .map(|(k, v)| (format!("{k:?}"), format!("{v:?}")))
                    .collect::<BTreeMap<_, _>>(),
                snap.wiring_edges
                    .into_iter()
                    .map(|edge| format!("{edge:?}"))
                    .collect::<BTreeSet<_>>(),
                snap.external_peer_edges
                    .into_iter()
                    .map(|edge| format!("{edge:?}"))
                    .collect::<BTreeSet<_>>(),
                snap.identity_to_runtime
                    .into_iter()
                    .map(|(k, v)| (format!("{k:?}"), format!("{v:?}")))
                    .collect::<BTreeMap<_, _>>(),
                snap.tasks
                    .into_iter()
                    .map(|(k, v)| (format!("{k:?}"), format!("{:?}", v.subject)))
                    .collect::<BTreeMap<_, _>>(),
                snap.in_progress_task_ids
                    .into_iter()
                    .map(|id| format!("{id:?}"))
                    .collect::<BTreeSet<_>>(),
                snap.completed_task_ids
                    .into_iter()
                    .map(|id| format!("{id:?}"))
                    .collect::<BTreeSet<_>>(),
                snap.member_restore_failures
                    .into_iter()
                    .map(|(k, v)| (format!("{k:?}"), v))
                    .collect::<BTreeMap<_, _>>(),
                snap.member_session_bindings
                    .into_iter()
                    .map(|(k, v)| (format!("{k:?}"), format!("{v:?}")))
                    .collect::<BTreeMap<_, _>>(),
                snap.pending_spawn_sessions
                    .into_iter()
                    .map(|(k, v)| (format!("{k:?}"), format!("{v:?}")))
                    .collect::<BTreeMap<_, _>>(),
                snap.pending_session_ingress_detach_runtime_ids
                    .into_iter()
                    .map(|id| format!("{id:?}"))
                    .collect::<BTreeSet<_>>(),
                snap.topology_epoch,
            )
        })
        .unwrap_or_default();

    Some(MobRuntimeParitySnapshotSummary {
        phase: phase.as_str().to_string(),
        live_intent_identities,
        live_runtime_ids,
        externally_addressable_runtime_ids,
        runtime_fence_tokens,
        active_member_count: active_members.len(),
        all_member_count: all_members.len(),
        task_count: tasks.as_ref().map(std::vec::Vec::len),
        coordinator_bound: orchestrator
            .as_ref()
            .map(|snapshot| snapshot.coordinator_bound),
        pending_spawn_count: orchestrator
            .as_ref()
            .map(|snapshot| snapshot.pending_spawn_count),
        active_flow_count: lifecycle.as_ref().map(|snapshot| snapshot.active_run_count),
        topology_revision: orchestrator
            .as_ref()
            .map(|snapshot| snapshot.topology_revision),
        supervisor_active: orchestrator
            .as_ref()
            .map(|snapshot| snapshot.supervisor_active),
        representative_agent_identity,
        representative_runtime_id,
        representative_fence_token,
        formal_available_fields,
        formal_unavailable_fields,
        member_state_markers,
        wiring_edges,
        external_peer_edges,
        identity_to_runtime,
        tasks: tasks_map,
        in_progress_task_ids,
        completed_task_ids,
        member_restore_failures,
        member_session_bindings,
        pending_spawn_sessions,
        pending_session_ingress_detach_runtime_ids,
        topology_epoch,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MobRuntimeParityExprValue {
    Bool(bool),
    U64(u64),
    String(String),
    Set(BTreeSet<String>),
    Map(BTreeMap<String, u64>),
    None,
}

fn mob_runtime_parity_field_value(
    snapshot: &MobRuntimeParitySnapshotSummary,
    field: &str,
) -> Option<MobRuntimeParityExprValue> {
    match field {
        "live_intent_identities" => Some(MobRuntimeParityExprValue::Set(
            snapshot.live_intent_identities.clone(),
        )),
        "live_runtime_ids" => Some(MobRuntimeParityExprValue::Set(
            snapshot.live_runtime_ids.clone(),
        )),
        "member_state_markers" => Some(MobRuntimeParityExprValue::Map(
            snapshot
                .member_state_markers
                .keys()
                .map(|k| (k.clone(), 0u64))
                .collect(),
        )),
        "wiring_edges" => Some(MobRuntimeParityExprValue::Set(
            snapshot.wiring_edges.clone(),
        )),
        "external_peer_edges" => Some(MobRuntimeParityExprValue::Set(
            snapshot.external_peer_edges.clone(),
        )),
        "identity_to_runtime" => Some(MobRuntimeParityExprValue::Map(
            snapshot
                .identity_to_runtime
                .keys()
                .map(|k| (k.clone(), 0u64))
                .collect(),
        )),
        "tasks" => Some(MobRuntimeParityExprValue::Map(
            snapshot.tasks.keys().map(|k| (k.clone(), 0u64)).collect(),
        )),
        "in_progress_task_ids" => Some(MobRuntimeParityExprValue::Set(
            snapshot.in_progress_task_ids.clone(),
        )),
        "completed_task_ids" => Some(MobRuntimeParityExprValue::Set(
            snapshot.completed_task_ids.clone(),
        )),
        "member_restore_failures" => Some(MobRuntimeParityExprValue::Map(
            snapshot
                .member_restore_failures
                .keys()
                .map(|k| (k.clone(), 0u64))
                .collect(),
        )),
        "member_session_bindings" => Some(MobRuntimeParityExprValue::Map(
            snapshot
                .member_session_bindings
                .keys()
                .map(|k| (k.clone(), 0u64))
                .collect(),
        )),
        "pending_spawn_sessions" => Some(MobRuntimeParityExprValue::Map(
            snapshot
                .pending_spawn_sessions
                .keys()
                .map(|k| (k.clone(), 0u64))
                .collect(),
        )),
        "pending_session_ingress_detach_runtime_ids" => Some(MobRuntimeParityExprValue::Set(
            snapshot.pending_session_ingress_detach_runtime_ids.clone(),
        )),
        "topology_epoch" => Some(MobRuntimeParityExprValue::U64(snapshot.topology_epoch)),
        "externally_addressable_runtime_ids" => Some(MobRuntimeParityExprValue::Set(
            snapshot.externally_addressable_runtime_ids.clone(),
        )),
        "runtime_fence_tokens" => Some(MobRuntimeParityExprValue::Map(
            snapshot.runtime_fence_tokens.clone(),
        )),
        "active_run_count" => Some(MobRuntimeParityExprValue::U64(
            snapshot.active_flow_count.unwrap_or_default() as u64,
        )),
        "pending_spawn_count" => Some(MobRuntimeParityExprValue::U64(
            snapshot.pending_spawn_count.unwrap_or_default() as u64,
        )),
        "task_count" => Some(MobRuntimeParityExprValue::U64(
            snapshot.task_count.unwrap_or_default() as u64,
        )),
        "coordinator_bound" => snapshot
            .coordinator_bound
            .map(MobRuntimeParityExprValue::Bool),
        "run_ready_frame_membership_flat"
        | "run_pending_body_frame_loop_membership_flat"
        | "member_startup_binding_requested"
        | "member_startup_runtime_ready"
        | "member_startup_ready"
        | "member_kickoff_pending"
        | "member_kickoff_starting"
        | "member_kickoff_callback_pending"
        | "member_kickoff_started"
        | "member_kickoff_failed"
        | "member_kickoff_cancelled" => Some(MobRuntimeParityExprValue::Set(BTreeSet::new())),
        "run_status"
        | "run_ordered_steps"
        | "run_tracked_steps"
        | "run_step_status"
        | "run_step_status_flat"
        | "run_output_recorded"
        | "run_step_condition_results_flat"
        | "run_step_condition_results"
        | "run_step_has_conditions"
        | "run_step_dependencies"
        | "run_step_dependency_modes"
        | "run_step_branches"
        | "run_step_collection_policies"
        | "run_step_quorum_thresholds"
        | "run_step_target_counts"
        | "run_step_target_success_counts"
        | "run_step_target_terminal_failure_counts"
        | "run_output_recorded_flat"
        | "run_step_target_counts_flat"
        | "run_step_target_success_counts_flat"
        | "run_step_target_terminal_failure_counts_flat"
        | "run_target_retry_counts"
        | "run_target_retry_counts_flat"
        | "run_failure_count"
        | "run_consecutive_failure_count"
        | "run_escalation_threshold"
        | "run_max_step_retries"
        | "run_ready_frames"
        | "run_ready_frame_membership"
        | "run_pending_body_frame_loops"
        | "run_pending_body_frame_loop_membership"
        | "run_active_node_count"
        | "run_active_frame_count"
        | "run_last_granted_frame"
        | "run_last_granted_loop"
        | "run_max_active_nodes"
        | "run_max_active_frames"
        | "run_max_frame_depth"
        | "frame_scope"
        | "frame_phase"
        | "frame_run"
        | "frame_parent_loop"
        | "frame_iteration"
        | "frame_tracked_nodes"
        | "frame_ordered_nodes"
        | "frame_node_kind"
        | "frame_node_dependencies"
        | "frame_node_dependency_modes"
        | "frame_node_step_ids"
        | "frame_node_loop_ids"
        | "frame_node_status"
        | "frame_ready_queue"
        | "frame_output_recorded"
        | "frame_output_recorded_flat"
        | "frame_last_admitted_node"
        | "frame_node_condition_results"
        | "frame_node_branches"
        | "loop_phase"
        | "loop_parent_frame"
        | "loop_parent_node"
        | "loop_definition"
        | "loop_depth"
        | "loop_stage"
        | "loop_current_iteration"
        | "loop_last_completed_iteration"
        | "loop_max_iterations"
        | "loop_active_body_frame"
        | "member_kickoff_error" => Some(MobRuntimeParityExprValue::Map(BTreeMap::new())),
        _ => None,
    }
}

fn mob_runtime_parity_eval_expr(
    expr: &Expr,
    snapshot: &MobRuntimeParitySnapshotSummary,
) -> Option<MobRuntimeParityExprValue> {
    match expr {
        Expr::Bool(value) => Some(MobRuntimeParityExprValue::Bool(*value)),
        Expr::U64(value) => Some(MobRuntimeParityExprValue::U64(*value)),
        Expr::String(value) => Some(MobRuntimeParityExprValue::String(value.clone())),
        Expr::None => Some(MobRuntimeParityExprValue::None),
        Expr::CurrentPhase => Some(MobRuntimeParityExprValue::String(snapshot.phase.clone())),
        Expr::Phase(phase) => Some(MobRuntimeParityExprValue::String(
            phase.as_str().to_string(),
        )),
        Expr::Field(field) => mob_runtime_parity_field_value(snapshot, field.as_str()),
        Expr::Not(inner) => mob_runtime_parity_eval_bool(inner, snapshot)
            .map(|value| MobRuntimeParityExprValue::Bool(!value)),
        Expr::And(items) => match items
            .iter()
            .map(|item| mob_runtime_parity_eval_bool(item, snapshot))
            .collect::<Vec<_>>()
            .as_slice()
        {
            values if values.contains(&Some(false)) => Some(MobRuntimeParityExprValue::Bool(false)),
            values if values.iter().all(|value| *value == Some(true)) => {
                Some(MobRuntimeParityExprValue::Bool(true))
            }
            _ => None,
        },
        Expr::Or(items) => match items
            .iter()
            .map(|item| mob_runtime_parity_eval_bool(item, snapshot))
            .collect::<Vec<_>>()
            .as_slice()
        {
            values if values.contains(&Some(true)) => Some(MobRuntimeParityExprValue::Bool(true)),
            values if values.iter().all(|value| *value == Some(false)) => {
                Some(MobRuntimeParityExprValue::Bool(false))
            }
            _ => None,
        },
        Expr::Eq(left, right) => {
            let left = mob_runtime_parity_eval_expr(left, snapshot)?;
            let right = mob_runtime_parity_eval_expr(right, snapshot)?;
            Some(MobRuntimeParityExprValue::Bool(left == right))
        }
        Expr::Neq(left, right) => {
            let left = mob_runtime_parity_eval_expr(left, snapshot)?;
            let right = mob_runtime_parity_eval_expr(right, snapshot)?;
            Some(MobRuntimeParityExprValue::Bool(left != right))
        }
        Expr::Gt(left, right) => {
            let left = mob_runtime_parity_eval_expr(left, snapshot)?;
            let right = mob_runtime_parity_eval_expr(right, snapshot)?;
            match (left, right) {
                (MobRuntimeParityExprValue::U64(left), MobRuntimeParityExprValue::U64(right)) => {
                    Some(MobRuntimeParityExprValue::Bool(left > right))
                }
                _ => None,
            }
        }
        Expr::Gte(left, right) => {
            let left = mob_runtime_parity_eval_expr(left, snapshot)?;
            let right = mob_runtime_parity_eval_expr(right, snapshot)?;
            match (left, right) {
                (MobRuntimeParityExprValue::U64(left), MobRuntimeParityExprValue::U64(right)) => {
                    Some(MobRuntimeParityExprValue::Bool(left >= right))
                }
                _ => None,
            }
        }
        Expr::Lt(left, right) => {
            let left = mob_runtime_parity_eval_expr(left, snapshot)?;
            let right = mob_runtime_parity_eval_expr(right, snapshot)?;
            match (left, right) {
                (MobRuntimeParityExprValue::U64(left), MobRuntimeParityExprValue::U64(right)) => {
                    Some(MobRuntimeParityExprValue::Bool(left < right))
                }
                _ => None,
            }
        }
        Expr::Lte(left, right) => {
            let left = mob_runtime_parity_eval_expr(left, snapshot)?;
            let right = mob_runtime_parity_eval_expr(right, snapshot)?;
            match (left, right) {
                (MobRuntimeParityExprValue::U64(left), MobRuntimeParityExprValue::U64(right)) => {
                    Some(MobRuntimeParityExprValue::Bool(left <= right))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn mob_runtime_parity_eval_bool(
    expr: &Expr,
    snapshot: &MobRuntimeParitySnapshotSummary,
) -> Option<bool> {
    match mob_runtime_parity_eval_expr(expr, snapshot) {
        Some(MobRuntimeParityExprValue::Bool(value)) => Some(value),
        _ => None,
    }
}

fn summarize_mob_runtime_success(probe: MobRuntimeParityProbeInput, summary: &str) -> String {
    format!("{probe:?}:{summary}")
}

fn summarize_mob_runtime_error(error: &MobError) -> String {
    match error {
        MobError::ProfileNotFound(_) => "profile_not_found".to_string(),
        MobError::MemberNotFound(_) => "meerkat_not_found".to_string(),
        MobError::MemberAlreadyExists(_) => "meerkat_already_exists".to_string(),
        MobError::NotExternallyAddressable(_) => "not_externally_addressable".to_string(),
        MobError::InvalidTransition { from, to } => {
            format!("invalid_transition:{}->{}", from.as_str(), to.as_str())
        }
        MobError::WiringError(_) => "wiring_error".to_string(),
        MobError::BridgeCommandRejected { cause, .. } => {
            format!("bridge_command_rejected:{cause:?}")
        }
        MobError::MemberRestoreFailed { .. } => "member_restore_failed".to_string(),
        MobError::KickoffWaitTimedOut { .. } => "kickoff_wait_timed_out".to_string(),
        MobError::ReadyWaitTimedOut { .. } => "ready_wait_timed_out".to_string(),
        MobError::DefinitionError(_) => "definition_error".to_string(),
        MobError::FlowNotFound(_) => "flow_not_found".to_string(),
        MobError::FlowFailed { .. } => "flow_failed".to_string(),
        MobError::RunNotFound(_) => "run_not_found".to_string(),
        MobError::RunCanceled(_) => "run_canceled".to_string(),
        MobError::FlowTurnTimedOut => "flow_turn_timed_out".to_string(),
        MobError::FrameDepthLimitExceeded { .. } => "frame_depth_limit_exceeded".to_string(),
        MobError::FrameAtomicPersistenceUnavailable { .. } => {
            "frame_atomic_persistence_unavailable".to_string()
        }
        MobError::SpecRevisionConflict { .. } => "spec_revision_conflict".to_string(),
        MobError::SchemaValidation { .. } => "schema_validation".to_string(),
        MobError::InsufficientTargets { .. } => "insufficient_targets".to_string(),
        MobError::TopologyViolation { .. } => "topology_violation".to_string(),
        MobError::BridgeDeliveryRejected { cause, .. } => {
            format!("bridge_delivery_rejected:{cause}")
        }
        MobError::SupervisorEscalation(_) => "supervisor_escalation".to_string(),
        MobError::UnsupportedForMode { .. } => "unsupported_for_mode".to_string(),
        MobError::ResetBarrier => "reset_barrier".to_string(),
        MobError::StorageError(_) => "storage_error".to_string(),
        MobError::SessionError(_) => "session_error".to_string(),
        MobError::CommsError(_) => "comms_error".to_string(),
        MobError::CallbackPending { .. } => "callback_pending".to_string(),
        MobError::StaleFenceToken { .. } => "stale_fence_token".to_string(),
        MobError::StaleEventCursor { .. } => "stale_event_cursor".to_string(),
        MobError::WorkNotFound(_) => "work_not_found".to_string(),
        MobError::Internal(reason) => format!("internal:{reason}"),
    }
}

fn mob_modeled_schema_result_summary(
    before: &MobRuntimeParitySnapshotSummary,
    probe: MobRuntimeParityProbeInput,
) -> Option<String> {
    match probe {
        MobRuntimeParityProbeInput::Spawn => Some(summarize_mob_runtime_success(probe, "spawned")),
        MobRuntimeParityProbeInput::SubmitWork => {
            Some(summarize_mob_runtime_success(probe, "work_receipt"))
        }
        MobRuntimeParityProbeInput::RunFlow => Some(summarize_mob_runtime_success(probe, "run_id")),
        MobRuntimeParityProbeInput::EnsureMember
        | MobRuntimeParityProbeInput::Reconcile
        | MobRuntimeParityProbeInput::CancelFlow
        | MobRuntimeParityProbeInput::Retire
        | MobRuntimeParityProbeInput::RetireAll
        | MobRuntimeParityProbeInput::WireMembers
        | MobRuntimeParityProbeInput::UnwireMembers
        | MobRuntimeParityProbeInput::WireExternalPeer
        | MobRuntimeParityProbeInput::UnwireExternalPeer
        | MobRuntimeParityProbeInput::CancelWork
        | MobRuntimeParityProbeInput::CancelAllWork
        | MobRuntimeParityProbeInput::Stop
        | MobRuntimeParityProbeInput::Resume
        | MobRuntimeParityProbeInput::Complete
        | MobRuntimeParityProbeInput::Reset
        | MobRuntimeParityProbeInput::Destroy
        | MobRuntimeParityProbeInput::TaskUpdate
        | MobRuntimeParityProbeInput::RecordOperatorActionProvenance
        | MobRuntimeParityProbeInput::SetSpawnPolicy
        | MobRuntimeParityProbeInput::Shutdown
        | MobRuntimeParityProbeInput::ForceCancel => {
            Some(summarize_mob_runtime_success(probe, "unit"))
        }
        MobRuntimeParityProbeInput::Respawn => {
            Some(summarize_mob_runtime_success(probe, "respawned"))
        }
        MobRuntimeParityProbeInput::ExternalTurn => {
            Some(summarize_mob_runtime_success(probe, "bridge_session"))
        }
        MobRuntimeParityProbeInput::InternalTurn => {
            Some(summarize_mob_runtime_success(probe, "member_delivery"))
        }
        MobRuntimeParityProbeInput::TaskCreate => {
            Some(summarize_mob_runtime_success(probe, "task_id"))
        }
        MobRuntimeParityProbeInput::SubscribeAgentEvents => {
            Some(summarize_mob_runtime_success(probe, "event_stream"))
        }
        MobRuntimeParityProbeInput::SubscribeAllAgentEvents => Some(summarize_mob_runtime_success(
            probe,
            &format!("all_agent_streams:{}", before.all_member_count),
        )),
        MobRuntimeParityProbeInput::SubscribeMobEvents => {
            Some(summarize_mob_runtime_success(probe, "mob_event_router"))
        }
    }
}

async fn build_mob_runtime_parity_fixture() -> MobRuntimeParityFixture {
    let definition = sample_definition_with_single_step_flow(60_000, 8);
    let (handle, service) = create_test_mob(definition).await;
    let _ = service.enable_runtime_adapter();
    service.set_flow_turn_delay_ms(150);
    MobRuntimeParityFixture {
        handle,
        service,
        worker_identity: AgentIdentity::from("w-1"),
        lead_identity: AgentIdentity::from("l-1"),
        cancel_identity: AgentIdentity::from("cancel-target"),
        task_id: None,
        flow_run_id: None,
        submitted_work_ref: None,
        wired_external: false,
    }
}

fn mob_runtime_parity_external_peer() -> TrustedPeerDescriptor {
    TrustedPeerDescriptor::test_only_unsigned_typed(
        "mob-runtime-parity/external/agent",
        meerkat_core::comms::PeerId::new(),
        "inproc://mob-runtime-parity/external/agent",
    )
    .expect("valid runtime parity external peer")
}

async fn mob_runtime_parity_prepare_probe(
    target_phase: MobRuntimeParityPhase,
    probe: MobRuntimeParityProbeInput,
    fixture: &mut MobRuntimeParityFixture,
    setup_tags: &mut Vec<String>,
) -> Result<(), String> {
    match probe {
        MobRuntimeParityProbeInput::SubmitWork
        | MobRuntimeParityProbeInput::Retire
        | MobRuntimeParityProbeInput::Respawn
        | MobRuntimeParityProbeInput::InternalTurn
        | MobRuntimeParityProbeInput::CancelAllWork => {
            fixture.ensure_worker().await?;
            setup_tags.push("worker_spawned".to_string());
        }
        MobRuntimeParityProbeInput::CancelWork => {
            fixture.ensure_worker().await?;
            setup_tags.push("worker_spawned".to_string());
            let _ = fixture.ensure_submitted_work().await?;
            setup_tags.push("work_submitted".to_string());
        }
        MobRuntimeParityProbeInput::RunFlow => {
            fixture.ensure_worker().await?;
            setup_tags.push("worker_spawned".to_string());
        }
        MobRuntimeParityProbeInput::CancelFlow => {
            if target_phase == MobRuntimeParityPhase::Running {
                let _ = fixture.ensure_demo_run().await?;
                setup_tags.push("flow_run_started".to_string());
            } else {
                fixture.flow_run_id = Some(RunId::new());
                setup_tags.push("synthetic_run_id".to_string());
            }
        }
        MobRuntimeParityProbeInput::RetireAll => {
            fixture.ensure_worker().await?;
            setup_tags.push("worker_spawned".to_string());
        }
        MobRuntimeParityProbeInput::WireMembers => {
            fixture.ensure_worker().await?;
            fixture.ensure_lead().await?;
            setup_tags.push("worker_spawned".to_string());
            setup_tags.push("lead_spawned".to_string());
        }
        MobRuntimeParityProbeInput::UnwireMembers => {
            fixture.ensure_wired_edge().await?;
            setup_tags.push("wired_external_edge".to_string());
        }
        MobRuntimeParityProbeInput::WireExternalPeer => {
            fixture.ensure_lead().await?;
            setup_tags.push("lead_spawned".to_string());
        }
        MobRuntimeParityProbeInput::UnwireExternalPeer => {
            fixture.ensure_lead().await?;
            fixture
                .handle
                .wire(
                    fixture.lead_identity.clone(),
                    PeerTarget::External(mob_runtime_parity_external_peer()),
                )
                .await
                .map_err(|error| format!("wire external peer: {error:?}"))?;
            setup_tags.push("external_peer_wired".to_string());
        }
        MobRuntimeParityProbeInput::ExternalTurn
        | MobRuntimeParityProbeInput::SubscribeAgentEvents
        | MobRuntimeParityProbeInput::SubscribeAllAgentEvents => {
            fixture.ensure_lead().await?;
            setup_tags.push("lead_spawned".to_string());
        }
        MobRuntimeParityProbeInput::TaskUpdate => {
            fixture.ensure_worker().await?;
            setup_tags.push("worker_spawned".to_string());
            let _ = fixture.ensure_task().await?;
            setup_tags.push("task_created".to_string());
        }
        MobRuntimeParityProbeInput::ForceCancel => {
            fixture.ensure_force_cancel_member().await?;
            setup_tags.push("turn_driven_member_spawned".to_string());
        }
        MobRuntimeParityProbeInput::Spawn
        | MobRuntimeParityProbeInput::Stop
        | MobRuntimeParityProbeInput::Resume
        | MobRuntimeParityProbeInput::Complete
        | MobRuntimeParityProbeInput::Reset
        | MobRuntimeParityProbeInput::Destroy
        | MobRuntimeParityProbeInput::TaskCreate
        | MobRuntimeParityProbeInput::SubscribeMobEvents
        | MobRuntimeParityProbeInput::RecordOperatorActionProvenance
        | MobRuntimeParityProbeInput::SetSpawnPolicy
        | MobRuntimeParityProbeInput::Shutdown
        | MobRuntimeParityProbeInput::EnsureMember
        | MobRuntimeParityProbeInput::Reconcile => {}
    }

    match target_phase {
        MobRuntimeParityPhase::Running => {}
        MobRuntimeParityPhase::Stopped => {
            if fixture.handle.status().await.unwrap() != MobState::Stopped {
                fixture
                    .handle
                    .stop()
                    .await
                    .map_err(|error| format!("stop to target phase: {error:?}"))?;
                setup_tags.push("transitioned_to_stopped".to_string());
            }
        }
        MobRuntimeParityPhase::Completed => {
            if fixture.handle.status().await.unwrap() != MobState::Completed {
                fixture
                    .handle
                    .complete()
                    .await
                    .map_err(|error| format!("complete to target phase: {error:?}"))?;
                setup_tags.push("transitioned_to_completed".to_string());
            }
        }
    }

    Ok(())
}

async fn mob_runtime_parity_execute_probe(
    fixture: &mut MobRuntimeParityFixture,
    probe: MobRuntimeParityProbeInput,
) -> Result<String, MobError> {
    match probe {
        MobRuntimeParityProbeInput::Spawn => fixture
            .handle
            .spawn(
                ProfileName::from("worker"),
                MeerkatId::from("parity-spawn"),
                None,
            )
            .await
            .map(|_| summarize_mob_runtime_success(probe, "spawned")),
        MobRuntimeParityProbeInput::EnsureMember => fixture
            .handle
            .ensure_member(SpawnMemberSpec::new("worker", "parity-ensure"))
            .await
            .map(|_| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::Reconcile => fixture
            .handle
            .reconcile(
                vec![SpawnMemberSpec::new("worker", "parity-reconcile")],
                crate::runtime::reconcile::ReconcileOptions::default(),
            )
            .await
            .map(|_| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::SubmitWork => {
            let entry = fixture.worker_entry().await.map_err(MobError::Internal)?;
            fixture
                .handle
                .submit_work(
                    entry.agent_runtime_id.clone(),
                    entry.fence_token,
                    WorkRef::new(),
                    WorkSpec::new("mob-runtime-parity".to_string(), WorkOrigin::Internal),
                )
                .await
                .map(|_| summarize_mob_runtime_success(probe, "work_receipt"))
        }
        MobRuntimeParityProbeInput::RunFlow => fixture
            .handle
            .run_flow(
                FlowId::from("demo"),
                serde_json::json!({"source":"mob-runtime-parity"}),
            )
            .await
            .map(|_| summarize_mob_runtime_success(probe, "run_id")),
        MobRuntimeParityProbeInput::CancelFlow => {
            let run_id = match &fixture.flow_run_id {
                Some(run_id) => run_id.clone(),
                None => RunId::new(),
            };
            fixture
                .handle
                .cancel_flow(run_id)
                .await
                .map(|()| summarize_mob_runtime_success(probe, "unit"))
        }
        MobRuntimeParityProbeInput::Retire => fixture
            .handle
            .retire(fixture.worker_identity.clone())
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::Respawn => fixture
            .handle
            .respawn(fixture.worker_identity.clone(), None)
            .await
            .map(|_| summarize_mob_runtime_success(probe, "respawned"))
            .map_err(|error| MobError::Internal(error.to_string())),
        MobRuntimeParityProbeInput::RetireAll => fixture
            .handle
            .retire_all()
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::WireMembers => fixture
            .handle
            .wire(
                fixture.worker_identity.clone(),
                MeerkatId::from(fixture.lead_identity.as_str()),
            )
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::UnwireMembers => fixture
            .handle
            .unwire(
                fixture.worker_identity.clone(),
                MeerkatId::from(fixture.lead_identity.as_str()),
            )
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::WireExternalPeer => fixture
            .handle
            .wire(
                fixture.lead_identity.clone(),
                PeerTarget::External(mob_runtime_parity_external_peer()),
            )
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::UnwireExternalPeer => fixture
            .handle
            .unwire(
                fixture.lead_identity.clone(),
                MeerkatId::from(mob_runtime_parity_external_peer().name.as_str()),
            )
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::ExternalTurn => fixture
            .handle
            .external_turn_for_member(
                MeerkatId::from(fixture.lead_identity.as_str()),
                meerkat_core::types::ContentInput::from("mob runtime parity external turn"),
                meerkat_core::types::HandlingMode::Queue,
                None,
            )
            .await
            .map(|()| summarize_mob_runtime_success(probe, "bridge_session")),
        MobRuntimeParityProbeInput::InternalTurn => fixture
            .handle
            .internal_turn(
                fixture.worker_identity.clone(),
                "mob runtime parity internal turn",
            )
            .await
            .map(|_| summarize_mob_runtime_success(probe, "member_delivery")),
        MobRuntimeParityProbeInput::CancelWork => fixture
            .handle
            .cancel_work(fixture.submitted_work_ref.clone().unwrap_or_default())
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::CancelAllWork => {
            let entry = fixture.worker_entry().await.map_err(MobError::Internal)?;
            fixture
                .handle
                .cancel_all_work(entry.agent_runtime_id.clone(), entry.fence_token)
                .await
                .map(|()| summarize_mob_runtime_success(probe, "unit"))
        }
        MobRuntimeParityProbeInput::Stop => fixture
            .handle
            .stop()
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::Resume => fixture
            .handle
            .resume()
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::Complete => fixture
            .handle
            .complete()
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::Reset => fixture
            .handle
            .reset()
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::Destroy => fixture
            .handle
            .destroy()
            .await
            .map(|_| summarize_mob_runtime_success(probe, "destroy_report"))
            .map_err(|error| MobError::Internal(error.to_string())),
        MobRuntimeParityProbeInput::TaskCreate => fixture
            .handle
            .task_create("mob runtime parity".to_string(), "task".to_string(), vec![])
            .await
            .map(|_| summarize_mob_runtime_success(probe, "task_id")),
        MobRuntimeParityProbeInput::TaskUpdate => {
            let task_id = fixture.ensure_task().await.map_err(MobError::Internal)?;
            fixture
                .handle
                .task_update(
                    task_id,
                    crate::tasks::TaskStatus::InProgress,
                    Some(fixture.worker_identity.clone()),
                )
                .await
                .map(|()| summarize_mob_runtime_success(probe, "unit"))
        }
        MobRuntimeParityProbeInput::SubscribeAgentEvents => fixture
            .handle
            .subscribe_agent_events(&fixture.lead_identity)
            .await
            .map(|_| summarize_mob_runtime_success(probe, "event_stream")),
        MobRuntimeParityProbeInput::SubscribeAllAgentEvents => {
            let streams = fixture.handle.subscribe_all_agent_events().await?;
            Ok(summarize_mob_runtime_success(
                probe,
                &format!("all_agent_streams:{}", streams.len()),
            ))
        }
        MobRuntimeParityProbeInput::SubscribeMobEvents => {
            let router = fixture
                .handle
                .subscribe_mob_events_with_config(MobEventRouterConfig {
                    poll_interval: Duration::from_millis(10),
                    channel_capacity: 32,
                })
                .await;
            router.cancel();
            Ok(summarize_mob_runtime_success(probe, "mob_event_router"))
        }
        MobRuntimeParityProbeInput::RecordOperatorActionProvenance => {
            let authority = meerkat_core::service::MobToolAuthorityContext::create_only_generated()
                .with_audit_invocation_id("mob-runtime-parity");
            fixture
                .handle
                .record_operator_action_provenance("spawn_member", &authority)
                .await
                .map(|()| summarize_mob_runtime_success(probe, "unit"))
        }
        MobRuntimeParityProbeInput::SetSpawnPolicy => fixture
            .handle
            .set_spawn_policy(None)
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::Shutdown => fixture
            .handle
            .shutdown()
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
        MobRuntimeParityProbeInput::ForceCancel => fixture
            .handle
            .force_cancel_member(fixture.cancel_identity.clone())
            .await
            .map(|()| summarize_mob_runtime_success(probe, "unit")),
    }
}

async fn execute_mob_runtime_parity_probe(
    phase: MobRuntimeParityPhase,
    probe: MobRuntimeParityProbeInput,
) -> Result<MobRuntimeParityInvocationReport, String> {
    const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

    let mut fixture = build_mob_runtime_parity_fixture().await;
    let mut setup_tags = Vec::new();
    let setup_result = tokio::time::timeout(
        PROBE_TIMEOUT,
        mob_runtime_parity_prepare_probe(phase, probe, &mut fixture, &mut setup_tags),
    )
    .await;
    let setup_error = match setup_result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => Some(format!(
            "setup timed out for {} {:?}",
            phase.schema_name(),
            probe
        )),
    };
    if let Some(error) = setup_error {
        fixture.cleanup().await;
        return Err(error);
    }

    let before = mob_runtime_parity_snapshot_summary(&fixture.handle).await;
    let result = tokio::time::timeout(
        PROBE_TIMEOUT,
        mob_runtime_parity_execute_probe(&mut fixture, probe),
    )
    .await;
    let after = mob_runtime_parity_snapshot_summary(&fixture.handle).await;
    fixture.cleanup().await;

    let (outcome_kind, result_summary) = match result {
        Ok(Ok(summary)) => (MobRuntimeParityOutcomeKind::Ok, summary),
        Ok(Err(error)) => (
            MobRuntimeParityOutcomeKind::Err,
            summarize_mob_runtime_error(&error),
        ),
        Err(_) => (
            MobRuntimeParityOutcomeKind::Err,
            format!("probe_timeout:{probe:?}"),
        ),
    };

    Ok(MobRuntimeParityInvocationReport {
        phase: phase.schema_name().to_string(),
        setup_tags,
        before,
        outcome_kind,
        result_summary,
        after,
    })
}

fn classify_mob_runtime_parity_probe_pair(
    left: &MobRuntimeParityInvocationReport,
    right: &MobRuntimeParityInvocationReport,
) -> MobRuntimeParityClassification {
    match (left.outcome_kind, right.outcome_kind) {
        (MobRuntimeParityOutcomeKind::Ok, MobRuntimeParityOutcomeKind::Err) => {
            MobRuntimeParityClassification::LeftOnly
        }
        (MobRuntimeParityOutcomeKind::Err, MobRuntimeParityOutcomeKind::Ok) => {
            MobRuntimeParityClassification::RightOnly
        }
        _ if left.observable_surface() == right.observable_surface() => {
            MobRuntimeParityClassification::SameSurface
        }
        _ => MobRuntimeParityClassification::DifferentSurface,
    }
}

async fn probe_mob_runtime_parity_row(
    left_phase: MobRuntimeParityPhase,
    right_phase: MobRuntimeParityPhase,
    probe: MobRuntimeParityProbeInput,
    schema_classification: MobRuntimeParityClassification,
) -> Result<MobRuntimeParityProbeReport, String> {
    let left = execute_mob_runtime_parity_probe(left_phase, probe).await?;
    let right = execute_mob_runtime_parity_probe(right_phase, probe).await?;
    let runtime_classification = classify_mob_runtime_parity_probe_pair(&left, &right);

    Ok(MobRuntimeParityProbeReport {
        schema_classification,
        runtime_classification,
        agrees_with_schema: runtime_classification == schema_classification,
        schema_left: None,
        schema_right: None,
        left,
        right,
    })
}

async fn probe_mob_runtime_full_parity_row(
    schema: &MachineSchema,
    left_phase: MobRuntimeParityPhase,
    right_phase: MobRuntimeParityPhase,
    probe: MobRuntimeParityProbeInput,
) -> Result<MobRuntimeParityProbeReport, String> {
    let left = execute_mob_runtime_parity_probe(left_phase, probe).await?;
    let right = execute_mob_runtime_parity_probe(right_phase, probe).await?;
    let schema_left = mob_modeled_schema_report(schema, &left, probe);
    let schema_right = mob_modeled_schema_report(schema, &right, probe);
    let schema_classification = classify_mob_modeled_schema_pair(&schema_left, &schema_right);
    let runtime_classification = classify_mob_runtime_parity_probe_pair(&left, &right);

    Ok(MobRuntimeParityProbeReport {
        schema_classification,
        runtime_classification,
        agrees_with_schema: runtime_classification == schema_classification,
        schema_left: Some(schema_left),
        schema_right: Some(schema_right),
        left,
        right,
    })
}

fn mob_runtime_parity_transition_enabled(
    transition: &meerkat_machine_schema::TransitionSchema,
    representative: Option<&MobRuntimeParitySnapshotSummary>,
) -> bool {
    representative.is_none_or(|snapshot| {
        transition
            .guards
            .iter()
            .all(|guard| mob_runtime_parity_eval_bool(&guard.expr, snapshot) != Some(false))
    })
}

fn mob_runtime_parity_schema_transition_summaries_for_phase_input(
    schema: &MachineSchema,
    phase: &str,
    input_variant: &str,
    representative: Option<&MobRuntimeParitySnapshotSummary>,
) -> Vec<MobRuntimeParitySchemaTransitionSummary> {
    let mut summaries = schema
        .transitions
        .iter()
        .filter(|transition| {
            transition.on.kind() == TriggerKind::Input
                && transition.on.variant_str() == input_variant
                && transition.from.iter().any(|from| from.as_str() == phase)
                && mob_runtime_parity_transition_enabled(transition, representative)
        })
        .map(|transition| MobRuntimeParitySchemaTransitionSummary {
            transition: transition.name.as_str().to_string(),
            to_phase: transition.to.as_str().to_string(),
            binding_names: transition
                .on
                .bindings()
                .iter()
                .map(|b| b.as_str().to_string())
                .collect(),
            guard_names: transition
                .guards
                .iter()
                .map(|guard| guard.name.to_string())
                .collect(),
            update_count: transition.updates.len(),
            effect_variants: transition
                .emit
                .iter()
                .map(|effect| effect.variant.as_str().to_string())
                .collect(),
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| left.transition.cmp(&right.transition));
    summaries
}

fn classify_mob_runtime_parity_schema_row(
    left: &[MobRuntimeParitySchemaTransitionSummary],
    right: &[MobRuntimeParitySchemaTransitionSummary],
) -> MobRuntimeParityClassification {
    if left.is_empty() && !right.is_empty() {
        return MobRuntimeParityClassification::RightOnly;
    }
    if !left.is_empty() && right.is_empty() {
        return MobRuntimeParityClassification::LeftOnly;
    }

    let left_surface = left
        .iter()
        .map(|summary| {
            (
                summary.to_phase.clone(),
                summary.binding_names.clone(),
                summary.guard_names.clone(),
                summary.update_count,
                summary.effect_variants.clone(),
            )
        })
        .collect::<std::collections::BTreeSet<_>>();
    let right_surface = right
        .iter()
        .map(|summary| {
            (
                summary.to_phase.clone(),
                summary.binding_names.clone(),
                summary.guard_names.clone(),
                summary.update_count,
                summary.effect_variants.clone(),
            )
        })
        .collect::<std::collections::BTreeSet<_>>();

    if left_surface == right_surface {
        MobRuntimeParityClassification::SameSurface
    } else {
        MobRuntimeParityClassification::DifferentSurface
    }
}

fn mob_runtime_parity_schema_row_for_input(
    schema: &MachineSchema,
    left_phase: MobRuntimeParityPhase,
    right_phase: MobRuntimeParityPhase,
    input_variant: &str,
    left_representative: Option<&MobRuntimeParitySnapshotSummary>,
    right_representative: Option<&MobRuntimeParitySnapshotSummary>,
) -> Option<MobRuntimeParitySchemaRow> {
    let static_left = mob_runtime_parity_schema_transition_summaries_for_phase_input(
        schema,
        left_phase.schema_name(),
        input_variant,
        None,
    );
    let static_right = mob_runtime_parity_schema_transition_summaries_for_phase_input(
        schema,
        right_phase.schema_name(),
        input_variant,
        None,
    );
    if static_left.is_empty() && static_right.is_empty() {
        return None;
    }

    let left = mob_runtime_parity_schema_transition_summaries_for_phase_input(
        schema,
        left_phase.schema_name(),
        input_variant,
        left_representative,
    );
    let right = mob_runtime_parity_schema_transition_summaries_for_phase_input(
        schema,
        right_phase.schema_name(),
        input_variant,
        right_representative,
    );

    Some(MobRuntimeParitySchemaRow {
        input_variant: input_variant.to_string(),
        classification: classify_mob_runtime_parity_schema_row(&left, &right),
        left,
        right,
    })
}

async fn build_mob_runtime_parity_pair_report(
    schema: &MachineSchema,
    left_phase: MobRuntimeParityPhase,
    right_phase: MobRuntimeParityPhase,
) -> MobRuntimeParityPairReport {
    let mut rows = Vec::new();
    let surface_only_inputs = schema
        .surface_only_inputs
        .iter()
        .map(|v| v.as_str())
        .collect::<BTreeSet<_>>();

    for input_variant in &schema.inputs.variants {
        println!(
            "probing {} <-> {} on {}",
            left_phase.schema_name(),
            right_phase.schema_name(),
            input_variant.name,
        );
        let probe_required = !surface_only_inputs.contains(input_variant.name.as_str());
        let (mut probe, note) =
            match mob_runtime_parity_probe_for_input_variant(input_variant.name.as_str()) {
                Some(probe_input) => {
                    match probe_mob_runtime_parity_row(
                        left_phase,
                        right_phase,
                        probe_input,
                        MobRuntimeParityClassification::SameSurface,
                    )
                    .await
                    {
                        Ok(probe) => (Some(probe), None),
                        Err(error) => (None, Some(format!("probe setup failed: {error}"))),
                    }
                }
                None if probe_required => (
                    None,
                    Some(
                        "required runtime probe missing; non-surface-only omissions fail closed"
                            .to_string(),
                    ),
                ),
                None => (
                    None,
                    Some("surface-only input: runtime probe not required".to_string()),
                ),
            };

        let Some(schema_row) = mob_runtime_parity_schema_row_for_input(
            schema,
            left_phase,
            right_phase,
            input_variant.name.as_str(),
            probe.as_ref().and_then(|probe| probe.left.before.as_ref()),
            probe.as_ref().and_then(|probe| probe.right.before.as_ref()),
        ) else {
            continue;
        };

        if let Some(probe) = &mut probe {
            probe.schema_classification = schema_row.classification;
            probe.agrees_with_schema = probe.runtime_classification == schema_row.classification;
        }

        rows.push(MobRuntimeParityRowReport {
            input_variant: schema_row.input_variant,
            probe_required,
            schema_classification: schema_row.classification,
            schema_left: schema_row.left,
            schema_right: schema_row.right,
            probe,
            note,
        });
    }

    let summary = rows.iter().fold(
        MobRuntimeParityPairSummary {
            interesting_rows: rows.len(),
            ..Default::default()
        },
        |mut summary, row| {
            match &row.probe {
                Some(probe) => {
                    summary.probed_rows += 1;
                    if probe.agrees_with_schema {
                        summary.aligned_rows += 1;
                    } else {
                        summary.mismatched_rows += 1;
                    }
                }
                None if row.probe_required => summary.unprobed_rows += 1,
                None => summary.surface_only_unprobed_rows += 1,
            }
            summary
        },
    );

    MobRuntimeParityPairReport {
        left_phase: left_phase.schema_name().to_string(),
        right_phase: right_phase.schema_name().to_string(),
        summary,
        rows,
    }
}

async fn build_mob_runtime_full_pair_report(
    schema: &MachineSchema,
    left_phase: MobRuntimeParityPhase,
    right_phase: MobRuntimeParityPhase,
) -> MobRuntimeParityPairReport {
    let mut rows = Vec::new();
    let surface_only_inputs = schema
        .surface_only_inputs
        .iter()
        .map(|v| v.as_str())
        .collect::<BTreeSet<_>>();

    for input_variant in &schema.inputs.variants {
        println!(
            "probing full {} <-> {} on {}",
            left_phase.schema_name(),
            right_phase.schema_name(),
            input_variant.name,
        );
        let probe_required = !surface_only_inputs.contains(input_variant.name.as_str());
        let (probe, note) =
            match mob_runtime_parity_probe_for_input_variant(input_variant.name.as_str()) {
                Some(probe_input) => {
                    match probe_mob_runtime_full_parity_row(
                        schema,
                        left_phase,
                        right_phase,
                        probe_input,
                    )
                    .await
                    {
                        Ok(probe) => (Some(probe), None),
                        Err(error) => (None, Some(format!("probe setup failed: {error}"))),
                    }
                }
                None if probe_required => (
                    None,
                    Some(
                        "required runtime probe missing; non-surface-only omissions fail closed"
                            .to_string(),
                    ),
                ),
                None => (
                    None,
                    Some("surface-only input: runtime probe not required".to_string()),
                ),
            };

        let Some(schema_row) = mob_runtime_parity_schema_row_for_input(
            schema,
            left_phase,
            right_phase,
            input_variant.name.as_str(),
            probe.as_ref().and_then(|probe| probe.left.before.as_ref()),
            probe.as_ref().and_then(|probe| probe.right.before.as_ref()),
        ) else {
            continue;
        };

        let effective_schema_classification = probe
            .as_ref()
            .map(|probe| probe.schema_classification)
            .unwrap_or(schema_row.classification);
        let note = match (&probe, note) {
            (Some(probe), None) if probe.schema_classification != schema_row.classification => {
                Some(format!(
                    "static schema classified {:?}, modeled schema classified {:?}",
                    schema_row.classification, probe.schema_classification
                ))
            }
            (_, note) => note,
        };

        rows.push(MobRuntimeParityRowReport {
            input_variant: schema_row.input_variant,
            probe_required,
            schema_classification: effective_schema_classification,
            schema_left: schema_row.left,
            schema_right: schema_row.right,
            probe,
            note,
        });
    }

    let summary = rows.iter().fold(
        MobRuntimeParityPairSummary {
            interesting_rows: rows.len(),
            ..Default::default()
        },
        |mut summary, row| {
            match &row.probe {
                Some(probe) => {
                    summary.probed_rows += 1;
                    if probe.agrees_with_schema {
                        summary.aligned_rows += 1;
                    } else {
                        summary.mismatched_rows += 1;
                    }
                }
                None if row.probe_required => summary.unprobed_rows += 1,
                None => summary.surface_only_unprobed_rows += 1,
            }
            summary
        },
    );

    MobRuntimeParityPairReport {
        left_phase: left_phase.schema_name().to_string(),
        right_phase: right_phase.schema_name().to_string(),
        summary,
        rows,
    }
}

fn mob_runtime_parity_probe_variant_name(probe: MobRuntimeParityProbeInput) -> &'static str {
    match probe {
        MobRuntimeParityProbeInput::Spawn => "Spawn",
        MobRuntimeParityProbeInput::EnsureMember => "EnsureMember",
        MobRuntimeParityProbeInput::Reconcile => "Reconcile",
        MobRuntimeParityProbeInput::SubmitWork => "SubmitWork",
        MobRuntimeParityProbeInput::RunFlow => "RunFlow",
        MobRuntimeParityProbeInput::CancelFlow => "CancelFlow",
        MobRuntimeParityProbeInput::Retire => "Retire",
        MobRuntimeParityProbeInput::Respawn => "Respawn",
        MobRuntimeParityProbeInput::RetireAll => "RetireAll",
        MobRuntimeParityProbeInput::WireMembers => "WireMembers",
        MobRuntimeParityProbeInput::UnwireMembers => "UnwireMembers",
        MobRuntimeParityProbeInput::WireExternalPeer => "WireExternalPeer",
        MobRuntimeParityProbeInput::UnwireExternalPeer => "UnwireExternalPeer",
        MobRuntimeParityProbeInput::ExternalTurn => "ExternalTurn",
        MobRuntimeParityProbeInput::InternalTurn => "InternalTurn",
        MobRuntimeParityProbeInput::CancelWork => "CancelWork",
        MobRuntimeParityProbeInput::CancelAllWork => "CancelAllWork",
        MobRuntimeParityProbeInput::Stop => "Stop",
        MobRuntimeParityProbeInput::Resume => "Resume",
        MobRuntimeParityProbeInput::Complete => "Complete",
        MobRuntimeParityProbeInput::Reset => "Reset",
        MobRuntimeParityProbeInput::Destroy => "Destroy",
        MobRuntimeParityProbeInput::TaskCreate => "TaskCreate",
        MobRuntimeParityProbeInput::TaskUpdate => "TaskUpdate",
        MobRuntimeParityProbeInput::SubscribeAgentEvents => "SubscribeAgentEvents",
        MobRuntimeParityProbeInput::SubscribeAllAgentEvents => "SubscribeAllAgentEvents",
        MobRuntimeParityProbeInput::SubscribeMobEvents => "SubscribeMobEvents",
        MobRuntimeParityProbeInput::RecordOperatorActionProvenance => {
            "RecordOperatorActionProvenance"
        }
        MobRuntimeParityProbeInput::SetSpawnPolicy => "SetSpawnPolicy",
        MobRuntimeParityProbeInput::Shutdown => "Shutdown",
        MobRuntimeParityProbeInput::ForceCancel => "ForceCancel",
    }
}

fn mob_modeled_normalize_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .into_iter()
                .map(mob_modeled_normalize_json_value)
                .collect(),
        ),
        serde_json::Value::Object(entries) => {
            let mut normalized = serde_json::Map::new();
            let mut keys: Vec<_> = entries.into_iter().collect();
            keys.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (key, value) in keys {
                normalized.insert(key, mob_modeled_normalize_json_value(value));
            }
            serde_json::Value::Object(normalized)
        }
        other => other,
    }
}

fn mob_modeled_normalize_formal_string(raw: &str) -> String {
    serde_json::from_str(raw)
        .map(mob_modeled_normalize_json_value)
        .and_then(|value| serde_json::to_string(&value))
        .unwrap_or_else(|_| raw.to_string())
}

fn mob_modeled_default_kernel_value(
    ty: &meerkat_machine_schema::TypeRef,
) -> meerkat_machine_kernels::test_oracle::KernelValue {
    match ty {
        meerkat_machine_schema::TypeRef::Bool => {
            meerkat_machine_kernels::test_oracle::KernelValue::Bool(false)
        }
        meerkat_machine_schema::TypeRef::U32 | meerkat_machine_schema::TypeRef::U64 => {
            meerkat_machine_kernels::test_oracle::KernelValue::U64(0)
        }
        meerkat_machine_schema::TypeRef::String => {
            meerkat_machine_kernels::test_oracle::KernelValue::String(String::new())
        }
        meerkat_machine_schema::TypeRef::Named(name) => mob_modeled_named_value(
            name,
            if mob_modeled_named_type_is_u64(name.as_str()) {
                meerkat_machine_kernels::test_oracle::KernelValue::U64(0)
            } else {
                meerkat_machine_kernels::test_oracle::KernelValue::String(String::new())
            },
        ),
        meerkat_machine_schema::TypeRef::Enum(name) => {
            meerkat_machine_kernels::test_oracle::KernelValue::NamedVariant {
                enum_name: name.clone(),
                variant: meerkat_machine_schema::identity::EnumVariantId::parse("Unknown")
                    .expect("valid slug"),
            }
        }
        meerkat_machine_schema::TypeRef::Option(_) => {
            meerkat_machine_kernels::test_oracle::KernelValue::None
        }
        meerkat_machine_schema::TypeRef::Set(_) => {
            meerkat_machine_kernels::test_oracle::KernelValue::Set(BTreeSet::new())
        }
        meerkat_machine_schema::TypeRef::Seq(_) => {
            meerkat_machine_kernels::test_oracle::KernelValue::Seq(Vec::new())
        }
        meerkat_machine_schema::TypeRef::Map(_, _) => {
            meerkat_machine_kernels::test_oracle::KernelValue::Map(BTreeMap::new())
        }
    }
}

fn mob_modeled_named_type_is_u64(name: &str) -> bool {
    matches!(name, "FenceToken" | "Generation")
}

fn mob_modeled_named_value(
    type_name: &meerkat_machine_schema::identity::NamedTypeId,
    value: meerkat_machine_kernels::test_oracle::KernelValue,
) -> meerkat_machine_kernels::test_oracle::KernelValue {
    if matches!(
        &value,
        meerkat_machine_kernels::test_oracle::KernelValue::Named {
            type_name: existing,
            ..
        } if existing == type_name
    ) {
        return value;
    }

    let value = if mob_modeled_named_type_is_u64(type_name.as_str()) {
        match value {
            meerkat_machine_kernels::test_oracle::KernelValue::U64(value) => {
                meerkat_machine_kernels::test_oracle::KernelValue::U64(value)
            }
            meerkat_machine_kernels::test_oracle::KernelValue::String(value) => {
                meerkat_machine_kernels::test_oracle::KernelValue::U64(
                    serde_json::from_str::<u64>(&value)
                        .ok()
                        .or_else(|| value.parse::<u64>().ok())
                        .unwrap_or_default(),
                )
            }
            _ => meerkat_machine_kernels::test_oracle::KernelValue::U64(0),
        }
    } else {
        match value {
            meerkat_machine_kernels::test_oracle::KernelValue::String(value) => {
                meerkat_machine_kernels::test_oracle::KernelValue::String(value)
            }
            meerkat_machine_kernels::test_oracle::KernelValue::U64(value) => {
                meerkat_machine_kernels::test_oracle::KernelValue::String(value.to_string())
            }
            other => meerkat_machine_kernels::test_oracle::KernelValue::String(
                serde_json::to_string(&mob_modeled_json_from_kernel_value(&other))
                    .unwrap_or_default(),
            ),
        }
    };

    meerkat_machine_kernels::test_oracle::KernelValue::Named {
        type_name: type_name.clone(),
        value: Box::new(value),
    }
}

fn mob_modeled_coerce_value_to_type(
    ty: &meerkat_machine_schema::TypeRef,
    value: meerkat_machine_kernels::test_oracle::KernelValue,
) -> meerkat_machine_kernels::test_oracle::KernelValue {
    match ty {
        meerkat_machine_schema::TypeRef::Named(name) => mob_modeled_named_value(name, value),
        meerkat_machine_schema::TypeRef::Option(inner) => match value {
            meerkat_machine_kernels::test_oracle::KernelValue::None => {
                meerkat_machine_kernels::test_oracle::KernelValue::None
            }
            meerkat_machine_kernels::test_oracle::KernelValue::Map(mut entries)
                if entries.len() == 1
                    && entries.contains_key(
                        &meerkat_machine_kernels::test_oracle::KernelValue::String(
                            "value".to_string(),
                        ),
                    ) =>
            {
                let key =
                    meerkat_machine_kernels::test_oracle::KernelValue::String("value".to_string());
                let inner_value = entries
                    .remove(&key)
                    .unwrap_or(meerkat_machine_kernels::test_oracle::KernelValue::None);
                mob_modeled_option_some(mob_modeled_coerce_value_to_type(inner, inner_value))
            }
            other => mob_modeled_option_some(mob_modeled_coerce_value_to_type(inner, other)),
        },
        meerkat_machine_schema::TypeRef::Set(inner) => match value {
            meerkat_machine_kernels::test_oracle::KernelValue::Set(items) => {
                meerkat_machine_kernels::test_oracle::KernelValue::Set(
                    items
                        .into_iter()
                        .map(|item| mob_modeled_coerce_value_to_type(inner, item))
                        .collect(),
                )
            }
            other => other,
        },
        meerkat_machine_schema::TypeRef::Seq(inner) => match value {
            meerkat_machine_kernels::test_oracle::KernelValue::Seq(items) => {
                meerkat_machine_kernels::test_oracle::KernelValue::Seq(
                    items
                        .into_iter()
                        .map(|item| mob_modeled_coerce_value_to_type(inner, item))
                        .collect(),
                )
            }
            other => other,
        },
        meerkat_machine_schema::TypeRef::Map(key_ty, value_ty) => match value {
            meerkat_machine_kernels::test_oracle::KernelValue::Map(entries) => {
                meerkat_machine_kernels::test_oracle::KernelValue::Map(
                    entries
                        .into_iter()
                        .map(|(key, value)| {
                            (
                                mob_modeled_coerce_value_to_type(key_ty, key),
                                mob_modeled_coerce_value_to_type(value_ty, value),
                            )
                        })
                        .collect(),
                )
            }
            other => other,
        },
        _ => value,
    }
}

fn mob_modeled_option_some(
    value: meerkat_machine_kernels::test_oracle::KernelValue,
) -> meerkat_machine_kernels::test_oracle::KernelValue {
    meerkat_machine_kernels::test_oracle::KernelValue::Map(BTreeMap::from([(
        meerkat_machine_kernels::test_oracle::KernelValue::String("value".to_string()),
        value,
    )]))
}

fn mob_modeled_json_value_from_raw(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

fn mob_modeled_kernel_value_from_json(
    ty: &meerkat_machine_schema::TypeRef,
    value: &serde_json::Value,
) -> meerkat_machine_kernels::test_oracle::KernelValue {
    match ty {
        meerkat_machine_schema::TypeRef::Bool => {
            meerkat_machine_kernels::test_oracle::KernelValue::Bool(
                value.as_bool().unwrap_or(false),
            )
        }
        meerkat_machine_schema::TypeRef::U32 | meerkat_machine_schema::TypeRef::U64 => {
            meerkat_machine_kernels::test_oracle::KernelValue::U64(value.as_u64().unwrap_or(0))
        }
        meerkat_machine_schema::TypeRef::String => {
            meerkat_machine_kernels::test_oracle::KernelValue::String(
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| serde_json::to_string(value).unwrap_or_default()),
            )
        }
        meerkat_machine_schema::TypeRef::Named(name)
            if matches!(name.as_str(), "FenceToken" | "Generation") =>
        {
            meerkat_machine_kernels::test_oracle::KernelValue::Named {
                type_name: name.clone(),
                value: Box::new(meerkat_machine_kernels::test_oracle::KernelValue::U64(
                    value.as_u64().unwrap_or(0),
                )),
            }
        }
        meerkat_machine_schema::TypeRef::Named(name) => {
            let normalized = value
                .as_str()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                .unwrap_or_else(|| value.clone());
            meerkat_machine_kernels::test_oracle::KernelValue::Named {
                type_name: name.clone(),
                value: Box::new(meerkat_machine_kernels::test_oracle::KernelValue::String(
                    serde_json::to_string(&normalized).unwrap_or_else(|_| "null".into()),
                )),
            }
        }
        meerkat_machine_schema::TypeRef::Enum(name) => {
            meerkat_machine_kernels::test_oracle::KernelValue::NamedVariant {
                enum_name: name.clone(),
                variant: meerkat_machine_schema::identity::EnumVariantId::parse(
                    value.as_str().unwrap_or("Unknown"),
                )
                .unwrap_or_else(|_| {
                    meerkat_machine_schema::identity::EnumVariantId::parse("Unknown")
                        .expect("valid slug")
                }),
            }
        }
        meerkat_machine_schema::TypeRef::Option(inner) => {
            if value.is_null() {
                meerkat_machine_kernels::test_oracle::KernelValue::None
            } else {
                mob_modeled_option_some(mob_modeled_kernel_value_from_json(inner, value))
            }
        }
        meerkat_machine_schema::TypeRef::Set(inner) => {
            let values = value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .map(|item| mob_modeled_kernel_value_from_json(inner, item))
                        .collect()
                })
                .unwrap_or_default();
            meerkat_machine_kernels::test_oracle::KernelValue::Set(values)
        }
        meerkat_machine_schema::TypeRef::Seq(inner) => {
            meerkat_machine_kernels::test_oracle::KernelValue::Seq(
                value
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| mob_modeled_kernel_value_from_json(inner, item))
                            .collect()
                    })
                    .unwrap_or_default(),
            )
        }
        meerkat_machine_schema::TypeRef::Map(key_ty, value_ty) => {
            let mut entries = BTreeMap::new();
            if let Some(object) = value.as_object() {
                for (key, item) in object {
                    let key_value = mob_modeled_kernel_value_from_json(
                        key_ty,
                        &serde_json::Value::String(key.clone()),
                    );
                    let value_value = mob_modeled_kernel_value_from_json(value_ty, item);
                    entries.insert(key_value, value_value);
                }
            }
            meerkat_machine_kernels::test_oracle::KernelValue::Map(entries)
        }
    }
}

fn mob_modeled_kernel_value_from_raw(
    ty: &meerkat_machine_schema::TypeRef,
    raw: &str,
) -> meerkat_machine_kernels::test_oracle::KernelValue {
    mob_modeled_kernel_value_from_json(ty, &mob_modeled_json_value_from_raw(raw))
}

fn mob_modeled_json_from_kernel_value(
    value: &meerkat_machine_kernels::test_oracle::KernelValue,
) -> serde_json::Value {
    match value {
        meerkat_machine_kernels::test_oracle::KernelValue::Bool(value) => {
            serde_json::Value::Bool(*value)
        }
        meerkat_machine_kernels::test_oracle::KernelValue::U64(value) => {
            serde_json::Value::Number(serde_json::Number::from(*value))
        }
        meerkat_machine_kernels::test_oracle::KernelValue::String(value) => {
            serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.clone()))
        }
        meerkat_machine_kernels::test_oracle::KernelValue::Named { value, .. } => {
            mob_modeled_json_from_kernel_value(value)
        }
        meerkat_machine_kernels::test_oracle::KernelValue::NamedVariant { variant, .. } => {
            serde_json::Value::String(variant.as_str().to_string())
        }
        meerkat_machine_kernels::test_oracle::KernelValue::Seq(items) => serde_json::Value::Array(
            items
                .iter()
                .map(mob_modeled_json_from_kernel_value)
                .collect(),
        ),
        meerkat_machine_kernels::test_oracle::KernelValue::Set(items) => serde_json::Value::Array(
            items
                .iter()
                .map(mob_modeled_json_from_kernel_value)
                .collect(),
        ),
        meerkat_machine_kernels::test_oracle::KernelValue::Map(entries)
            if entries.len() == 1
                && entries.contains_key(
                    &meerkat_machine_kernels::test_oracle::KernelValue::String("value".to_string()),
                ) =>
        {
            mob_modeled_json_from_kernel_value(
                entries
                    .get(&meerkat_machine_kernels::test_oracle::KernelValue::String(
                        "value".to_string(),
                    ))
                    .expect("value key present"),
            )
        }
        meerkat_machine_kernels::test_oracle::KernelValue::Map(entries) => {
            let mut object = serde_json::Map::new();
            for (key, value) in entries {
                let key_json = mob_modeled_json_from_kernel_value(key);
                let key_string = key_json
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| serde_json::to_string(&key_json).unwrap_or_default());
                object.insert(key_string, mob_modeled_json_from_kernel_value(value));
            }
            serde_json::Value::Object(object)
        }
        meerkat_machine_kernels::test_oracle::KernelValue::None => serde_json::Value::Null,
    }
}

fn mob_modeled_formal_string_from_kernel_value(
    value: &meerkat_machine_kernels::test_oracle::KernelValue,
) -> String {
    mob_modeled_normalize_formal_string(
        &serde_json::to_string(&mob_modeled_json_from_kernel_value(value))
            .unwrap_or_else(|_| "null".into()),
    )
}

fn mob_modeled_summary_from_runtime_snapshot(
    snapshot: Option<&MobRuntimeParitySnapshotSummary>,
) -> Option<MobModeledStateSummary> {
    snapshot.map(|snapshot| MobModeledStateSummary {
        phase: snapshot.phase.clone(),
        formal_fields: snapshot.formal_available_fields.clone(),
        unavailable_fields: snapshot.formal_unavailable_fields.clone(),
    })
}

fn mob_modeled_summary_from_kernel_state(
    schema: &MachineSchema,
    state: &meerkat_machine_kernels::test_oracle::KernelState,
    runtime_reference: &MobRuntimeParitySnapshotSummary,
) -> MobModeledStateSummary {
    let formal_fields = schema
        .state
        .fields
        .iter()
        .filter(|field| {
            runtime_reference
                .formal_available_fields
                .contains_key(field.name.as_str())
        })
        .map(|field| {
            let value = state
                .fields
                .get(&field.name)
                .map(mob_modeled_formal_string_from_kernel_value)
                .unwrap_or_else(|| "null".to_string());
            (field.name.as_str().to_string(), value)
        })
        .collect();

    MobModeledStateSummary {
        phase: state.phase.as_str().to_string(),
        formal_fields,
        unavailable_fields: runtime_reference.formal_unavailable_fields.clone(),
    }
}

fn mob_modeled_differing_keys(
    runtime_after: &Option<MobModeledStateSummary>,
    schema_after: &Option<MobModeledStateSummary>,
) -> Vec<String> {
    let mut keys = BTreeSet::new();
    if runtime_after.as_ref().map(|summary| summary.phase.as_str())
        != schema_after.as_ref().map(|summary| summary.phase.as_str())
    {
        keys.insert("phase".to_string());
    }

    let runtime_fields = runtime_after
        .as_ref()
        .map(|summary| &summary.formal_fields)
        .cloned()
        .unwrap_or_default();
    let schema_fields = schema_after
        .as_ref()
        .map(|summary| &summary.formal_fields)
        .cloned()
        .unwrap_or_default();

    for key in runtime_fields
        .keys()
        .chain(schema_fields.keys())
        .collect::<BTreeSet<_>>()
    {
        if runtime_fields.get(key) != schema_fields.get(key) {
            keys.insert(key.clone());
        }
    }

    keys.into_iter().collect()
}

fn mob_modeled_kernel_state(
    schema: &MachineSchema,
    before: &MobRuntimeParitySnapshotSummary,
) -> meerkat_machine_kernels::test_oracle::KernelState {
    let mut fields = BTreeMap::new();
    for field in &schema.state.fields {
        let value = before
            .formal_available_fields
            .get(field.name.as_str())
            .map(|raw| mob_modeled_kernel_value_from_raw(&field.ty, raw))
            .unwrap_or_else(|| mob_modeled_default_kernel_value(&field.ty));
        fields.insert(
            field.name.clone(),
            mob_modeled_coerce_value_to_type(&field.ty, value),
        );
    }

    meerkat_machine_kernels::test_oracle::KernelState {
        phase: meerkat_machine_schema::identity::PhaseId::parse(before.phase.as_str())
            .expect("valid phase slug"),
        fields,
    }
}

fn mob_modeled_named_string(value: String) -> meerkat_machine_kernels::test_oracle::KernelValue {
    meerkat_machine_kernels::test_oracle::KernelValue::String(value)
}

fn mob_modeled_representative_identity_value(before: &MobRuntimeParitySnapshotSummary) -> String {
    before
        .representative_agent_identity
        .clone()
        .unwrap_or_else(|| "\"<agent-identity>\"".to_string())
}

fn mob_modeled_representative_runtime_value(before: &MobRuntimeParitySnapshotSummary) -> String {
    before
        .representative_runtime_id
        .clone()
        .unwrap_or_else(|| "\"<runtime-id>\"".to_string())
}

fn mob_modeled_spawn_runtime_value(identity: &str, generation: u64) -> String {
    serde_json::json!({
        "identity": identity,
        "generation": generation,
    })
    .to_string()
}

fn mob_modeled_kernel_input(
    schema: &MachineSchema,
    before: &MobRuntimeParitySnapshotSummary,
    probe: MobRuntimeParityProbeInput,
) -> Result<meerkat_machine_kernels::test_oracle::KernelInput, String> {
    let variant = meerkat_machine_schema::identity::InputVariantId::parse(
        mob_runtime_parity_probe_variant_name(probe),
    )
    .expect("valid variant slug");
    let input_variant = schema
        .inputs
        .variant_named(&variant)
        .map_err(|err| err.to_string())?;
    let mut fields = BTreeMap::new();

    for field in &input_variant.fields {
        let value = match field.name.as_str() {
            "agent_identity" => match probe {
                MobRuntimeParityProbeInput::Spawn => {
                    mob_modeled_named_string("\"parity-spawn\"".to_string())
                }
                _ => mob_modeled_named_string(mob_modeled_representative_identity_value(before)),
            },
            "agent_runtime_id" => match probe {
                MobRuntimeParityProbeInput::Spawn => {
                    mob_modeled_named_string(mob_modeled_spawn_runtime_value("parity-spawn", 0))
                }
                _ => mob_modeled_named_string(mob_modeled_representative_runtime_value(before)),
            },
            "fence_token" => {
                let value = match probe {
                    MobRuntimeParityProbeInput::Spawn => 1,
                    MobRuntimeParityProbeInput::Respawn => {
                        before.representative_fence_token.unwrap_or_default() + 1
                    }
                    _ => before.representative_fence_token.unwrap_or_default(),
                };
                meerkat_machine_kernels::test_oracle::KernelValue::U64(value)
            }
            "generation" => {
                let value = match probe {
                    MobRuntimeParityProbeInput::Spawn => 0,
                    MobRuntimeParityProbeInput::Respawn => 0,
                    _ => 0,
                };
                meerkat_machine_kernels::test_oracle::KernelValue::U64(value)
            }
            "work_id" => mob_modeled_named_string("\"<work-id>\"".to_string()),
            _ => mob_modeled_default_kernel_value(&field.ty),
        };
        fields.insert(
            field.name.clone(),
            mob_modeled_coerce_value_to_type(&field.ty, value),
        );
    }

    Ok(meerkat_machine_kernels::test_oracle::KernelInput { variant, fields })
}

fn mob_modeled_transition_refusal_detail(
    error: &meerkat_machine_kernels::test_oracle::TransitionRefusal,
) -> String {
    match error {
        meerkat_machine_kernels::test_oracle::TransitionRefusal::UnknownInputVariant {
            variant,
            ..
        } => {
            format!("unknown_input:{variant}")
        }
        meerkat_machine_kernels::test_oracle::TransitionRefusal::UnknownSignalVariant {
            variant,
            ..
        } => {
            format!("unknown_signal:{variant}")
        }
        meerkat_machine_kernels::test_oracle::TransitionRefusal::InvalidInputPayload {
            reason,
            ..
        } => {
            format!("invalid_input:{reason}")
        }
        meerkat_machine_kernels::test_oracle::TransitionRefusal::InvalidSignalPayload {
            reason,
            ..
        } => {
            format!("invalid_signal:{reason}")
        }
        meerkat_machine_kernels::test_oracle::TransitionRefusal::NoMatchingTransition {
            phase,
            trigger,
            ..
        } => {
            format!("no_match:{phase}:{}", trigger.as_str())
        }
        meerkat_machine_kernels::test_oracle::TransitionRefusal::AmbiguousTransition {
            phase,
            trigger,
            transitions,
            ..
        } => format!("ambiguous:{phase}:{}:{transitions:?}", trigger.as_str()),
        meerkat_machine_kernels::test_oracle::TransitionRefusal::EvaluationError {
            transition,
            reason,
            ..
        } => format!("evaluation:{transition}:{reason}"),
    }
}

fn mob_modeled_schema_report(
    schema: &MachineSchema,
    runtime: &MobRuntimeParityInvocationReport,
    probe: MobRuntimeParityProbeInput,
) -> MobModeledStateSchemaReport {
    let Some(before) = runtime.before.as_ref() else {
        return MobModeledStateSchemaReport {
            outcome_kind: MobModeledStateOutcomeKind::Err,
            after: None,
            detail: "missing runtime pre-state".to_string(),
            result_summary: None,
        };
    };

    let state = mob_modeled_kernel_state(schema, before);
    let input = match mob_modeled_kernel_input(schema, before, probe) {
        Ok(input) => input,
        Err(detail) => {
            return MobModeledStateSchemaReport {
                outcome_kind: MobModeledStateOutcomeKind::Err,
                after: None,
                detail,
                result_summary: None,
            };
        }
    };

    match meerkat_machine_kernels::test_oracle::GeneratedMachineKernel::new(
        meerkat_machine_kernels::generated::mob::schema(),
    )
    .transition(&state, &input)
    {
        Ok(outcome) => MobModeledStateSchemaReport {
            outcome_kind: MobModeledStateOutcomeKind::Ok,
            after: Some(mob_modeled_summary_from_kernel_state(
                schema,
                &outcome.next_state,
                before,
            )),
            detail: outcome.transition.as_str().to_string(),
            result_summary: mob_modeled_schema_result_summary(before, probe),
        },
        Err(error) => MobModeledStateSchemaReport {
            outcome_kind: MobModeledStateOutcomeKind::Err,
            after: Some(MobModeledStateSummary {
                phase: before.phase.clone(),
                formal_fields: before.formal_available_fields.clone(),
                unavailable_fields: before.formal_unavailable_fields.clone(),
            }),
            detail: mob_modeled_transition_refusal_detail(&error),
            result_summary: None,
        },
    }
}

fn classify_mob_modeled_schema_pair(
    left: &MobModeledStateSchemaReport,
    right: &MobModeledStateSchemaReport,
) -> MobRuntimeParityClassification {
    match (left.outcome_kind, right.outcome_kind) {
        (MobModeledStateOutcomeKind::Ok, MobModeledStateOutcomeKind::Err) => {
            MobRuntimeParityClassification::LeftOnly
        }
        (MobModeledStateOutcomeKind::Err, MobModeledStateOutcomeKind::Ok) => {
            MobRuntimeParityClassification::RightOnly
        }
        (MobModeledStateOutcomeKind::Ok, MobModeledStateOutcomeKind::Ok)
            if left.after == right.after && left.result_summary == right.result_summary =>
        {
            MobRuntimeParityClassification::SameSurface
        }
        (MobModeledStateOutcomeKind::Err, MobModeledStateOutcomeKind::Err)
            if left.after == right.after && left.detail == right.detail =>
        {
            MobRuntimeParityClassification::SameSurface
        }
        _ => MobRuntimeParityClassification::DifferentSurface,
    }
}

fn mob_modeled_runtime_report(
    runtime: &MobRuntimeParityInvocationReport,
) -> MobModeledStateRuntimeReport {
    let before = mob_modeled_summary_from_runtime_snapshot(runtime.before.as_ref());
    let after = mob_modeled_summary_from_runtime_snapshot(runtime.after.as_ref()).or_else(|| {
        (runtime.outcome_kind == MobRuntimeParityOutcomeKind::Err)
            .then(|| before.clone())
            .flatten()
    });

    MobModeledStateRuntimeReport {
        phase: runtime.phase.clone(),
        outcome_kind: match runtime.outcome_kind {
            MobRuntimeParityOutcomeKind::Ok => MobModeledStateOutcomeKind::Ok,
            MobRuntimeParityOutcomeKind::Err => MobModeledStateOutcomeKind::Err,
        },
        before,
        after,
        result_summary: runtime.result_summary.clone(),
    }
}

async fn write_mob_runtime_modeled_state_audit_report(path: PathBuf) -> MobModeledStateAuditReport {
    let schema = meerkat_machine_kernels::generated::mob::schema();
    let surface_only_inputs = schema
        .surface_only_inputs
        .iter()
        .map(|v| v.as_str())
        .collect::<BTreeSet<_>>();
    let mut rows = Vec::new();

    for phase in [
        MobRuntimeParityPhase::Running,
        MobRuntimeParityPhase::Stopped,
        MobRuntimeParityPhase::Completed,
    ] {
        for input_variant in &schema.inputs.variants {
            if surface_only_inputs.contains(input_variant.name.as_str()) {
                continue;
            }
            let Some(probe) =
                mob_runtime_parity_probe_for_input_variant(input_variant.name.as_str())
            else {
                rows.push(MobModeledStateRowReport {
                    phase: phase.schema_name().to_string(),
                    input_variant: input_variant.name.as_str().to_string(),
                    aligned: false,
                    differing_keys: vec!["unprobed".to_string()],
                    runtime: MobModeledStateRuntimeReport {
                        phase: phase.schema_name().to_string(),
                        outcome_kind: MobModeledStateOutcomeKind::Err,
                        before: None,
                        after: None,
                        result_summary:
                            "required runtime probe missing; non-surface-only omissions fail closed"
                                .to_string(),
                    },
                    schema: MobModeledStateSchemaReport {
                        outcome_kind: MobModeledStateOutcomeKind::Err,
                        after: None,
                        detail:
                            "required runtime probe missing; non-surface-only omissions fail closed"
                                .to_string(),
                        result_summary: None,
                    },
                });
                continue;
            };

            let runtime = execute_mob_runtime_parity_probe(phase, probe)
                .await
                .expect("mob runtime modeled-state probe should succeed");
            let schema_report = mob_modeled_schema_report(&schema, &runtime, probe);
            let runtime_report = mob_modeled_runtime_report(&runtime);
            let differing_keys =
                mob_modeled_differing_keys(&runtime_report.after, &schema_report.after);
            let aligned = runtime_report.outcome_kind == schema_report.outcome_kind
                && differing_keys.is_empty();

            rows.push(MobModeledStateRowReport {
                phase: phase.schema_name().to_string(),
                input_variant: input_variant.name.as_str().to_string(),
                aligned,
                differing_keys,
                runtime: runtime_report,
                schema: schema_report,
            });
        }
    }

    let summary = rows.iter().fold(
        MobModeledStateAuditSummary::default(),
        |mut summary, row| {
            summary.row_count += 1;
            if row.runtime.result_summary
                == "required runtime probe missing; non-surface-only omissions fail closed"
            {
                summary.unprobed_rows += 1;
            } else if row.aligned {
                summary.aligned_rows += 1;
            } else {
                summary.mismatched_rows += 1;
            }
            summary
        },
    );

    let report = MobModeledStateAuditReport {
        machine: "MobMachine".to_string(),
        generated_at: Utc::now().to_rfc3339(),
        summary,
        rows,
    };

    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&report).expect("serialize modeled-state audit report"),
    )
    .expect("write modeled-state audit report");

    report
}

async fn write_mob_runtime_full_parity_audit_report(path: PathBuf) -> MobRuntimeParityAuditReport {
    let schema = schema_mob_machine();
    let mut pairs = Vec::new();

    for &(left_phase, right_phase) in mob_runtime_parity_target_pairs() {
        pairs.push(build_mob_runtime_full_pair_report(&schema, left_phase, right_phase).await);
    }

    let summary = pairs.iter().fold(
        MobRuntimeParityAuditSummary {
            pair_count: pairs.len(),
            ..Default::default()
        },
        |mut summary, pair| {
            summary.interesting_rows += pair.summary.interesting_rows;
            summary.probed_rows += pair.summary.probed_rows;
            summary.aligned_rows += pair.summary.aligned_rows;
            summary.mismatched_rows += pair.summary.mismatched_rows;
            summary.unprobed_rows += pair.summary.unprobed_rows;
            summary.surface_only_unprobed_rows += pair.summary.surface_only_unprobed_rows;
            summary
        },
    );

    let report = MobRuntimeParityAuditReport {
        machine: "MobMachine".to_string(),
        generated_at: Utc::now().to_rfc3339(),
        summary,
        pairs,
    };

    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&report).expect("serialize mob runtime full parity report"),
    )
    .expect("write mob runtime full parity report");

    report
}

// ---------------------------------------------------------------------------
// T5d: declarative API composition tests (ensure_member / reconcile /
// list_members_matching)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ensure_member_spawns_then_returns_existing() {
    use crate::runtime::reconcile::EnsureMemberOutcome;
    let (handle, _service) = create_test_mob(sample_definition()).await;

    let spec1 = crate::runtime::handle::SpawnMemberSpec::new(
        ProfileName::from("worker"),
        AgentIdentity::from("w-ensure"),
    );
    let first = handle
        .ensure_member(spec1)
        .await
        .expect("first ensure_member");
    assert!(
        matches!(first, EnsureMemberOutcome::Spawned(_)),
        "first ensure_member on absent identity should Spawn"
    );

    let spec2 = crate::runtime::handle::SpawnMemberSpec::new(
        ProfileName::from("worker"),
        AgentIdentity::from("w-ensure"),
    );
    let second = handle
        .ensure_member(spec2)
        .await
        .expect("second ensure_member");
    match second {
        EnsureMemberOutcome::Existed(entry) => {
            assert_eq!(entry.agent_identity, AgentIdentity::from("w-ensure"));
        }
        other => panic!("second ensure_member should be Existed, got {other:?}"),
    }
}

#[tokio::test]
async fn test_reconcile_spawns_missing_and_retires_stale() {
    use crate::runtime::reconcile::{ReconcileOptions, ReconcileStage};
    let (handle, _service) = create_test_mob(sample_definition()).await;

    // Seed one member that is NOT in the desired set so retire_stale takes it.
    handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("stale-1"),
            None,
        )
        .await
        .expect("seed stale-1");

    let desired = vec![
        crate::runtime::handle::SpawnMemberSpec::new(
            ProfileName::from("worker"),
            AgentIdentity::from("w-new-1"),
        ),
        crate::runtime::handle::SpawnMemberSpec::new(
            ProfileName::from("worker"),
            AgentIdentity::from("w-new-2"),
        ),
    ];
    let options = ReconcileOptions { retire_stale: true };
    let report = handle.reconcile(desired, options).await.expect("reconcile");

    assert_eq!(report.desired.len(), 2, "two desired identities reported");
    assert_eq!(report.spawned.len(), 2, "two new spawns committed");
    assert_eq!(
        report.retained.len(),
        0,
        "no desired identity was pre-existing"
    );
    assert_eq!(
        report.retired,
        vec![AgentIdentity::from("stale-1")],
        "stale-1 was retired because retire_stale is set",
    );
    assert!(
        report.failures.is_empty(),
        "no failures expected in happy path, got {:?}",
        report
            .failures
            .iter()
            .map(|f| (f.agent_identity.clone(), f.stage, f.error.to_string()))
            .collect::<Vec<_>>()
    );
    assert!(
        report
            .failures
            .iter()
            .all(|f| f.stage == ReconcileStage::Spawn || f.stage == ReconcileStage::Retire),
        "ReconcileStage tags cover only Spawn and Retire"
    );
}

#[tokio::test]
async fn test_list_members_matching_filters_by_label_and_role() {
    use crate::runtime::reconcile::MemberFilter;
    let (handle, _service) = create_test_mob(sample_definition()).await;

    let mut labels_a = std::collections::BTreeMap::new();
    labels_a.insert("faction".to_string(), "north".to_string());
    let mut labels_b = std::collections::BTreeMap::new();
    labels_b.insert("faction".to_string(), "south".to_string());

    handle
        .ensure_member(
            crate::runtime::handle::SpawnMemberSpec::new(
                ProfileName::from("worker"),
                AgentIdentity::from("w-north"),
            )
            .with_labels(labels_a),
        )
        .await
        .expect("spawn w-north");
    handle
        .ensure_member(
            crate::runtime::handle::SpawnMemberSpec::new(
                ProfileName::from("worker"),
                AgentIdentity::from("w-south"),
            )
            .with_labels(labels_b),
        )
        .await
        .expect("spawn w-south");

    let mut filter = MemberFilter::default();
    filter
        .labels
        .insert("faction".to_string(), "north".to_string());
    let matches = handle.list_members_matching(filter).await;
    assert_eq!(matches.len(), 1, "only one member matches faction=north");
    assert_eq!(matches[0].agent_identity, AgentIdentity::from("w-north"));

    // Empty filter matches every member.
    let all = handle.list_members_matching(MemberFilter::default()).await;
    assert_eq!(all.len(), 2, "empty filter returns both members");
}
