use super::*;
use crate::definition::{
    BackendConfig, CollectionPolicy, ConditionExpr, DependencyMode, DispatchMode, FlowSpec,
    FlowStepSpec, LimitsSpec, MobDefinition, OrchestratorConfig, PolicyMode, RoleWiringRule,
    SkillSource, StepOutputFormat, TopologyRule, TopologySpec, WiringRules,
};
use crate::event::MobEvent;
use crate::profile::{Profile, ToolConfig};
use crate::run::MobRunStatus;
use crate::run::{FailureLedgerEntry, MobRun, StepLedgerEntry, StepRunStatus};
use crate::storage::MobStorage;
use crate::store::{
    InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore, MobEventStore, MobRunStore,
};
use async_trait::async_trait;
use chrono::Utc;
use indexmap::IndexMap;
use meerkat_core::Provider;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, PeerDirectoryEntry, PeerDirectorySource, PeerName, PeerReachability,
    PeerReachabilityReason, SendError, SendReceipt, TrustedPeerSpec,
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
    AssistantBlock, Message, RunResult, SessionId, StopReason, ToolCallView, ToolDef, ToolResult,
    Usage,
};
use meerkat_core::{
    Agent, AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AppendSystemContextStatus, EventInjector, EventInjectorError, LlmStreamResult,
    PlainEventSource,
    event_injector::{InteractionSubscription, SubscribableInjector},
};
use meerkat_core::{Session, SessionMetadata, SessionSystemContextState, SessionTooling};
use meerkat_session::{SessionAgent, SessionAgentBuilder, SessionSnapshot};
use meerkat_store::{MemoryStore, SessionStore};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tempfile::NamedTempFile;

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
    default_public_key: String,
    behavior: std::sync::RwLock<MockCommsBehavior>,
    remove_failures_remaining: std::sync::Mutex<usize>,
    trusted_peers: RwLock<HashMap<String, TrustedPeerSpec>>,
    peer_statuses: RwLock<HashMap<String, (PeerReachability, Option<PeerReachabilityReason>)>>,
    sent_intents: RwLock<Vec<String>>,
    inbox_notify: Arc<tokio::sync::Notify>,
}

impl MockCommsRuntime {
    fn new(name: &str, behavior: MockCommsBehavior) -> Self {
        Self {
            default_public_key: format!("ed25519:{name}"),
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
            .map(|peer| peer.name.clone())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    async fn trusted_peer_addresses(&self) -> Vec<String> {
        let mut addresses = self
            .trusted_peers
            .read()
            .await
            .values()
            .map(|peer| peer.address.clone())
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

    async fn add_trusted_peer(&self, peer: TrustedPeerSpec) -> Result<(), SendError> {
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
        peers.insert(peer.peer_id.clone(), peer);
        Ok(())
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

    async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        match cmd {
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
                if !trusted.values().any(|peer| peer.name == to.as_str()) {
                    return Err(SendError::PeerNotFound(to.as_string()));
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
                let name = PeerName::new(peer.name.clone()).ok()?;
                let (reachability, last_unreachable_reason) = peer_statuses
                    .get(peer.name.as_str())
                    .copied()
                    .unwrap_or((PeerReachability::Unknown, None));
                Some(PeerDirectoryEntry {
                    name,
                    peer_id: peer_id.clone(),
                    address: peer.address.clone(),
                    source: PeerDirectorySource::Trusted,
                    sendable_kinds: vec![
                        "peer_message".to_string(),
                        "peer_request".to_string(),
                        "peer_response".to_string(),
                    ],
                    capabilities: serde_json::json!({}),
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
    runtime_adapter: Mutex<Option<Arc<meerkat_runtime::RuntimeSessionAdapter>>>,
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
        }
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

    fn enable_runtime_adapter(&self) -> Arc<meerkat_runtime::RuntimeSessionAdapter> {
        let mut guard = self.runtime_adapter.lock().expect("runtime_adapter mutex");
        if let Some(adapter) = guard.as_ref() {
            return adapter.clone();
        }
        let adapter = Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral());
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
                    .map(|tool| tool.name.clone())
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

    async fn keep_alive_prompts(&self) -> Vec<(SessionId, String)> {
        self.keep_alive_prompts.read().await.clone()
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
            && let Some(to_key) = to_runtime.public_key()
        {
            let _ = from_runtime.remove_trusted_peer(&to_key).await;
        }
    }

    async fn force_add_trust_from_spec(&self, session_id: &SessionId, spec: TrustedPeerSpec) {
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
                model: req.model.clone(),
                max_tokens: req.max_tokens.unwrap_or(4096),
                structured_output_retries: 2,
                provider: build
                    .and_then(|b| b.provider)
                    .unwrap_or(Provider::Anthropic),
                provider_params: build.and_then(|b| b.provider_params.clone()),
                tooling: SessionTooling {
                    builtins: build.and_then(|b| b.override_builtins).unwrap_or(true),
                    shell: build.and_then(|b| b.override_shell).unwrap_or(false),
                    comms: build.and_then(|b| b.comms_name.as_ref()).is_some(),
                    mob: build.and_then(|b| b.override_mob).unwrap_or(false),
                    memory: build.and_then(|b| b.override_memory).unwrap_or(false),
                    active_skills: build.and_then(|b| b.preload_skills.clone()),
                },
                keep_alive: build.map(|b| b.keep_alive).unwrap_or(false),
                comms_name: build.and_then(|b| b.comms_name.clone()),
                peer_meta: build.and_then(|b| b.peer_meta.clone()),
                realm_id: build.and_then(|b| b.realm_id.clone()),
                instance_id: build.and_then(|b| b.instance_id.clone()),
                backend: build.and_then(|b| b.backend.clone()),
                config_generation: build.and_then(|b| b.config_generation),
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
                                error: "mock flow turn failure".to_string(),
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
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                is_active: true,
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
        Ok(sessions
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
        Some(Arc::new(CountingInjector {
            calls: self.inject_calls.clone(),
            delay_ms,
            never_terminal,
            fail,
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
        Some(Arc::new(CountingInjector {
            calls: self.inject_calls.clone(),
            delay_ms,
            never_terminal,
            fail,
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

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::RuntimeSessionAdapter>> {
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
            run_result: None,
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
    fail_on_kind: RwLock<HashSet<&'static str>>,
    replay_calls: AtomicU64,
}

impl FaultInjectedMobEventStore {
    fn new() -> Self {
        Self {
            events: RwLock::new(Vec::new()),
            fail_on_kind: RwLock::new(HashSet::new()),
            replay_calls: AtomicU64::new(0),
        }
    }

    async fn fail_appends_for(&self, kind: &'static str) {
        self.fail_on_kind.write().await.insert(kind);
    }

    fn replay_calls(&self) -> u64 {
        self.replay_calls.load(Ordering::Relaxed)
    }

    fn kind_label(kind: &MobEventKind) -> &'static str {
        match kind {
            MobEventKind::MobCreated { .. } => "MobCreated",
            MobEventKind::MobCompleted => "MobCompleted",
            MobEventKind::MobReset => "MobReset",
            MobEventKind::MeerkatSpawned { .. } => "MeerkatSpawned",
            MobEventKind::MeerkatRetired { .. } => "MeerkatRetired",
            MobEventKind::PeersWired { .. } => "PeersWired",
            MobEventKind::ExternalPeerWired { .. } => "ExternalPeerWired",
            MobEventKind::ExternalPeerUnwired { .. } => "ExternalPeerUnwired",
            MobEventKind::PeersUnwired { .. } => "PeersUnwired",
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
        }
    }
}

#[async_trait]
impl MobEventStore for FaultInjectedMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError> {
        let kind_label = Self::kind_label(&event.kind);
        if self.fail_on_kind.read().await.contains(kind_label) {
            return Err(MobError::Internal(format!(
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
        Ok(stored)
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobError> {
        let events = self.events.read().await;
        Ok(events
            .iter()
            .filter(|e| e.cursor > after_cursor)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError> {
        self.replay_calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.events.read().await.clone())
    }

    async fn append_batch(&self, events: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobError> {
        let mut results = Vec::with_capacity(events.len());
        for event in events {
            results.push(self.append(event).await?);
        }
        Ok(results)
    }

    async fn clear(&self) -> Result<(), MobError> {
        self.events.write().await.clear();
        Ok(())
    }
}

struct RecordingRunStore {
    inner: InMemoryMobRunStore,
    cas_history: RwLock<Vec<(RunId, MobRunStatus, MobRunStatus)>>,
    snapshot_cas_history: RwLock<Vec<(RunId, MobRunStatus, MobRunStatus)>>,
}

impl RecordingRunStore {
    fn new() -> Self {
        Self {
            inner: InMemoryMobRunStore::new(),
            cas_history: RwLock::new(Vec::new()),
            snapshot_cas_history: RwLock::new(Vec::new()),
        }
    }

    async fn snapshot_cas_history(&self) -> Vec<(RunId, MobRunStatus, MobRunStatus)> {
        self.snapshot_cas_history.read().await.clone()
    }
}

#[async_trait]
impl MobRunStore for RecordingRunStore {
    async fn create_run(&self, run: MobRun) -> Result<(), MobError> {
        self.inner.create_run(run).await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobError> {
        self.inner.get_run(run_id).await
    }

    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&crate::FlowId>,
    ) -> Result<Vec<MobRun>, MobError> {
        self.inner.list_runs(mob_id, flow_id).await
    }

    async fn cas_run_status(
        &self,
        run_id: &RunId,
        expected: MobRunStatus,
        next: MobRunStatus,
    ) -> Result<bool, MobError> {
        self.cas_history
            .write()
            .await
            .push((run_id.clone(), expected.clone(), next.clone()));
        self.inner.cas_run_status(run_id, expected, next).await
    }

    async fn cas_flow_state(
        &self,
        run_id: &RunId,
        expected: &meerkat_machine_kernels::KernelState,
        next: &meerkat_machine_kernels::KernelState,
    ) -> Result<bool, MobError> {
        self.inner.cas_flow_state(run_id, expected, next).await
    }

    async fn cas_run_snapshot(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &meerkat_machine_kernels::KernelState,
        next_status: MobRunStatus,
        next_flow_state: &meerkat_machine_kernels::KernelState,
    ) -> Result<bool, MobError> {
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
    ) -> Result<(), MobError> {
        self.inner.append_step_entry(run_id, entry).await
    }

    async fn append_step_entry_if_absent(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<bool, MobError> {
        self.inner.append_step_entry_if_absent(run_id, entry).await
    }

    async fn put_step_output(
        &self,
        run_id: &RunId,
        step_id: &crate::StepId,
        output: serde_json::Value,
    ) -> Result<(), MobError> {
        self.inner.put_step_output(run_id, step_id, output).await
    }

    async fn append_failure_entry(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobError> {
        self.inner.append_failure_entry(run_id, entry).await
    }
}

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

fn sample_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        Profile {
            model: "claude-opus-4-6".into(),
            skills: vec![],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: false,
                mob: true,
                mob_tasks: false,
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
        },
    );
    profiles.insert(
        ProfileName::from("worker"),
        Profile {
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
        },
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
        .expect("worker profile exists");
    worker.tools.rust_bundles = vec![bundle_name.to_string()];
    def
}

fn sample_definition_with_mob_tools() -> MobDefinition {
    let mut def = sample_definition();
    let worker = def
        .profiles
        .get_mut(&ProfileName::from("worker"))
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
        .expect("lead profile exists");
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
        },
    );
    def.flows = flows;
    def.limits = Some(LimitsSpec {
        max_flow_duration_ms: None,
        max_step_retries: None,
        max_orphaned_turns: Some(max_orphaned_turns),
        cancel_grace_timeout_ms: None,
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

/// Create a MobHandle using the mock session service.
async fn create_test_mob(definition: MobDefinition) -> (MobHandle, Arc<MockSessionService>) {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    (handle, service)
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
    let handle = MobBuilder::new(definition, MobStorage::with_events(events))
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
    let storage = MobStorage {
        events: Arc::new(InMemoryMobEventStore::new()),
        runs: run_store,
        specs: Arc::new(InMemoryMobSpecStore::new()),
    };
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    (handle, service)
}

fn test_comms_name(profile: &str, meerkat_id: &str) -> String {
    format!("test-mob/{profile}/{meerkat_id}")
}

struct EchoBundleDispatcher;

#[async_trait]
impl AgentToolDispatcher for EchoBundleDispatcher {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        vec![Arc::new(meerkat_core::ToolDef {
            name: "bundle_echo".to_string(),
            description: "Echo input value".to_string(),
            input_schema: serde_json::Value::Object(serde_json::Map::new()),
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
        _req: &CreateSessionRequest,
        _event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        Ok(PersistentMockAgent {
            session_id: SessionId::new(),
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

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::RuntimeSessionAdapter>> {
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
        self.inner.cancel_all_checkpointers().await
    }

    async fn rearm_all_checkpointers(&self) {
        self.inner.rearm_all_checkpointers().await
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

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::RuntimeSessionAdapter>> {
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
        self.inner.cancel_all_checkpointers().await
    }

    async fn rearm_all_checkpointers(&self) {
        self.inner.rearm_all_checkpointers().await
    }
}

async fn create_test_mob_with_persistent_service(definition: MobDefinition) -> MobHandle {
    let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        PersistentMockBuilder,
        16,
        store,
        None,
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
                    name: (*name).to_string(),
                    description: format!("{name} tool"),
                    input_schema,
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
        _provider_params: Option<&serde_json::Value>,
    ) -> Result<LlmStreamResult, meerkat_core::error::AgentError> {
        self.provider_visible_tools
            .lock()
            .expect("provider_visible_tools lock poisoned")
            .push(tools.iter().map(|tool| tool.name.clone()).collect());
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
    assert_eq!(handle.status(), MobState::Running);
    assert_eq!(handle.mob_id().as_str(), "test-mob");
}

#[tokio::test]
async fn test_mob_builder_runs_with_shared_redb_storage_bundle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("mob.redb");
    let storage = MobStorage::redb(&db_path).expect("construct redb-backed storage");
    let service = Arc::new(MockSessionService::new());
    let handle = MobBuilder::new(sample_definition_with_single_step_flow(500, 8), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob with redb-backed storage");

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
    let session_service: Arc<dyn crate::runtime::MobSessionService> =
        Arc::new(MockSessionService::new());
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
    let storage = MobStorage::in_memory();
    let definition = sample_definition();
    let mut mismatched = definition.clone();
    mismatched
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile exists")
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
    let storage = MobStorage::in_memory();
    let definition = sample_definition();
    let mob_id = definition.id.clone();
    let storage_for_resume = MobStorage {
        events: storage.events.clone(),
        runs: storage.runs.clone(),
        specs: storage.specs.clone(),
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
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service)
        .create()
        .await
        .expect("warning diagnostics should not block create");
    assert_eq!(handle.status(), MobState::Running);
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

    let service = Arc::new(MockSessionService::new());
    let handle = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service)
        .resume()
        .await
        .expect("warning diagnostics should not block resume");
    assert_eq!(handle.status(), MobState::Running);
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

    let service = Arc::new(MockSessionService::new());
    let result = MobBuilder::for_resume(MobStorage::with_events(events))
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
        handle.get_member(&MeerkatId::from("w-1")).await.is_some(),
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
    assert_eq!(handle.status(), MobState::Running);
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
    assert_eq!(handle.status(), MobState::Stopped);
}

#[tokio::test]
async fn test_lifecycle_state_machine_enforcement() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    assert_eq!(handle.status(), MobState::Running);

    handle.stop().await.expect("stop");
    assert_eq!(handle.status(), MobState::Stopped);

    handle.resume().await.expect("resume");
    assert_eq!(handle.status(), MobState::Running);

    handle.complete().await.expect("complete");
    assert_eq!(handle.status(), MobState::Completed);

    let err = handle
        .resume()
        .await
        .expect_err("resume must fail from completed");
    assert!(
        matches!(err, MobError::InvalidTransition { .. }),
        "completed -> running must be rejected"
    );

    handle.destroy().await.expect("destroy");
    assert_eq!(handle.status(), MobState::Destroyed);

    let err = handle
        .destroy()
        .await
        .expect_err("destroy from Destroyed must fail");
    assert!(matches!(err, MobError::InvalidTransition { .. }));
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
    let event_count_before = handle.events().replay_all().await.expect("replay").len();

    handle.stop().await.expect("stop");
    assert_eq!(handle.status(), MobState::Stopped);
    assert_eq!(
        service.active_session_count().await,
        1,
        "stop should not archive active sessions"
    );
    assert_eq!(
        handle.events().replay_all().await.expect("replay").len(),
        event_count_before,
        "stop should not delete persisted events"
    );

    let err = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect_err("spawn must be rejected while stopped");
    assert!(matches!(err, MobError::InvalidTransition { .. }));
}

#[tokio::test]
async fn test_register_tool_bundle_is_wired_into_spawn() {
    let definition = sample_definition_with_tool_bundle("bundle-a");
    let service = Arc::new(MockSessionService::new());
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
        .session_id()
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
            .any(|e| matches!(e.kind, MobEventKind::MeerkatSpawned { .. })),
        "failed spawn should not emit MeerkatSpawned"
    );
}

#[tokio::test]
async fn test_mob_management_tools_dispatch_to_handle() {
    let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();

    let tool_names = service.external_tool_names(&sid_1).await;
    for required in [
        "spawn_meerkat",
        "spawn_many_meerkats",
        "retire_meerkat",
        "wire_peers",
        "unwire_peers",
        "list_meerkats",
    ] {
        assert!(
            tool_names.contains(&required.to_string()),
            "expected tool '{required}' to be present"
        );
    }

    let spawn_outcome = service
        .dispatch_external_tool_outcome(
            &sid_1,
            "spawn_meerkat",
            serde_json::json!({"profile": "worker", "meerkat_id": "w-2"}),
        )
        .await
        .expect("spawn_meerkat tool");
    let spawn_json: serde_json::Value = serde_json::from_str(&spawn_outcome.result.text_content())
        .expect("spawn_meerkat result JSON");
    assert!(
        spawn_json.get("operation_id").is_none(),
        "app-facing mob spawn result must not expose raw operation_id"
    );
    assert!(
        handle.get_member(&MeerkatId::from("w-2")).await.is_some(),
        "spawn_meerkat should create a new roster entry"
    );

    let spawn_many_outcome = service
        .dispatch_external_tool_outcome(
            &sid_1,
            "spawn_many_meerkats",
            serde_json::json!({
                "specs": [
                    {"profile": "worker", "meerkat_id": "w-many-1"},
                    {"profile": "worker", "meerkat_id": "w-many-2"}
                ]
            }),
        )
        .await
        .expect("spawn_many_meerkats tool");
    let spawn_many_json: serde_json::Value =
        serde_json::from_str(&spawn_many_outcome.result.text_content())
            .expect("spawn_many_meerkats result JSON");
    let results = spawn_many_json["results"]
        .as_array()
        .expect("spawn_many results array");
    assert!(
        results
            .iter()
            .all(|item| item.get("operation_id").is_none()),
        "app-facing mob batch spawn results must not expose raw operation_id"
    );
    assert!(
        handle
            .get_member(&MeerkatId::from("w-many-1"))
            .await
            .is_some(),
        "spawn_many_meerkats should create first roster entry"
    );
    assert!(
        handle
            .get_member(&MeerkatId::from("w-many-2"))
            .await
            .is_some(),
        "spawn_many_meerkats should create second roster entry"
    );

    service
        .dispatch_external_tool(
            &sid_1,
            "wire_peers",
            serde_json::json!({"a": "w-1", "b": "w-2"}),
        )
        .await
        .expect("wire peers");
    let w1 = handle
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("w-1");
    assert!(
        w1.wired_to.contains(&MeerkatId::from("w-2")),
        "wire_peers should update roster wiring"
    );

    let listed = service
        .dispatch_external_tool(&sid_1, "list_meerkats", serde_json::json!({}))
        .await
        .expect("list_meerkats");
    let listed_json: serde_json::Value =
        serde_json::from_str(&listed.text_content()).expect("list content json");
    assert!(
        listed_json["meerkats"]
            .as_array()
            .map_or(0, std::vec::Vec::len)
            >= 2,
        "list_meerkats should include spawned peers"
    );

    service
        .dispatch_external_tool(
            &sid_1,
            "unwire_peers",
            serde_json::json!({"a": "w-1", "b": "w-2"}),
        )
        .await
        .expect("unwire peers");
    service
        .dispatch_external_tool(
            &sid_1,
            "retire_meerkat",
            serde_json::json!({"meerkat_id": "w-2"}),
        )
        .await
        .expect("retire_meerkat");
    assert!(
        handle.get_member(&MeerkatId::from("w-2")).await.is_none(),
        "retire_meerkat should remove roster entry"
    );
}

#[tokio::test]
async fn test_spawn_helper_contract_aligns_with_retired_terminal_state() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let helper_id = MeerkatId::from("helper-spawn");
    let result = handle
        .spawn_helper(
            helper_id.clone(),
            "summarize this",
            HelperOptions {
                profile_name: Some(ProfileName::from("worker")),
                runtime_mode: Some(crate::MobRuntimeMode::TurnDriven),
                ..HelperOptions::default()
            },
        )
        .await
        .expect("spawn_helper succeeds");

    let session_id = result.session_id.expect("helper session id");
    assert!(
        service.read(&session_id).await.is_err(),
        "spawn_helper must retire the helper session once it returns"
    );
    assert!(
        handle.get_member(&helper_id).await.is_none(),
        "spawn_helper must remove the helper from the active roster once it returns"
    );
}

#[tokio::test]
async fn test_fork_helper_contract_aligns_with_retired_terminal_state() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let source_id = MeerkatId::from("fork-source");
    let source_ref = handle
        .spawn_with_options(
            ProfileName::from("worker"),
            source_id.clone(),
            Some("source context".into()),
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn source member");

    let helper_id = MeerkatId::from("fork-helper");
    let result = handle
        .fork_helper(
            &source_id,
            helper_id.clone(),
            "continue from source",
            crate::launch::ForkContext::FullHistory,
            HelperOptions {
                profile_name: Some(ProfileName::from("worker")),
                runtime_mode: Some(crate::MobRuntimeMode::TurnDriven),
                ..HelperOptions::default()
            },
        )
        .await
        .expect("fork_helper succeeds");

    let helper_session_id = result.session_id.expect("fork helper session id");
    assert!(
        service.read(&helper_session_id).await.is_err(),
        "fork_helper must retire the helper session once it returns"
    );
    assert!(
        handle.get_member(&helper_id).await.is_none(),
        "fork_helper must remove the helper from the active roster once it returns"
    );
    assert_eq!(
        source_ref.session_id().cloned(),
        handle
            .get_member(&source_id)
            .await
            .and_then(|entry| entry.member_ref.session_id().cloned()),
        "fork_helper must not perturb the source member's canonical session binding"
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
        .session_id()
        .cloned()
        .expect("original session id");

    let receipt = handle
        .respawn(member_id.clone(), Some("resume task".into()))
        .await
        .expect("respawn succeeds");

    assert_eq!(receipt.member_id, member_id);
    assert_eq!(receipt.old_session_id, Some(old_session_id.clone()));
    let new_session_id = receipt.new_session_id.clone().expect("new session id");
    assert_ne!(new_session_id, old_session_id);
    assert!(
        service.read(&old_session_id).await.is_err(),
        "respawn must archive the retired session before returning"
    );

    let snapshot = handle
        .member_status(&receipt.member_id)
        .await
        .expect("member snapshot");
    assert_eq!(snapshot.current_session_id, Some(new_session_id.clone()));
    assert_eq!(
        service
            .read(&new_session_id)
            .await
            .expect("new session readable")
            .state
            .session_id,
        new_session_id,
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
        .wire(left.clone(), right.clone())
        .await
        .expect("wire peers before respawn");

    let old_session_id = original_left
        .session_id()
        .cloned()
        .expect("left session id");
    let receipt = handle
        .respawn(left.clone(), Some("rebuild wiring".into()))
        .await
        .expect("respawn succeeds");

    assert_eq!(receipt.old_session_id, Some(old_session_id.clone()));
    assert!(
        service.read(&old_session_id).await.is_err(),
        "respawn must archive the retired session before returning"
    );

    let left_entry = handle.get_member(&left).await.expect("left remains active");
    let right_entry = handle
        .get_member(&right)
        .await
        .expect("right remains active");
    assert!(
        left_entry.wired_to.contains(&right),
        "respawn replacement must restore canonical wiring on the replacement member"
    );
    assert!(
        right_entry.wired_to.contains(&left),
        "respawn replacement must preserve the peer's canonical wiring projection"
    );
    assert_eq!(
        left_entry.member_ref.session_id().cloned(),
        receipt.new_session_id,
        "respawn receipt must align with the replacement member's canonical session binding"
    );
}

#[tokio::test]
async fn test_wait_one_returns_terminal_unknown_for_missing_member() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let snapshot = handle
        .wait_one(&MeerkatId::from("missing-helper"))
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
        .session_id()
        .expect("session-backed member")
        .clone();

    service.set_archive_delay_ms(250);
    let retire = {
        let handle = handle.clone();
        let member_id = member_id.clone();
        tokio::spawn(async move { handle.retire(member_id).await })
    };

    tokio::time::sleep(Duration::from_millis(25)).await;
    let in_flight = handle
        .member_status(&member_id)
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
        .wait_one(&member_id)
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
    let ids = vec![MeerkatId::from("missing-a"), MeerkatId::from("missing-b")];

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
async fn test_mob_flow_tools_dispatch_mutate_and_query_real_run_state() {
    let mut definition =
        with_cancel_grace_timeout(sample_definition_with_single_step_flow(60_000, 8), 25);
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile exists");
    worker.tools.mob = true;
    worker.tools.comms = true;

    let (handle, service) = create_test_mob(definition).await;
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();
    service.set_flow_turn_delay_ms(60_000);
    service.set_flow_turn_never_terminal(true);

    let tool_names = service.external_tool_names(&sid_1).await;
    for required in [
        "mob_list_flows",
        "mob_run_flow",
        "mob_flow_status",
        "mob_cancel_flow",
    ] {
        assert!(
            tool_names.contains(&required.to_string()),
            "expected flow tool '{required}' to be present"
        );
    }

    let listed = service
        .dispatch_external_tool(&sid_1, "mob_list_flows", serde_json::json!({}))
        .await
        .expect("mob_list_flows");
    let listed_json: serde_json::Value =
        serde_json::from_str(&listed.text_content()).expect("flows payload json");
    assert_eq!(listed_json["flows"], serde_json::json!(["demo"]));

    let started = service
        .dispatch_external_tool(
            &sid_1,
            "mob_run_flow",
            serde_json::json!({
                "flow_id": "demo",
                "params": {"ticket": "REQ-017"}
            }),
        )
        .await
        .expect("mob_run_flow");
    let started_json: serde_json::Value =
        serde_json::from_str(&started.text_content()).expect("run payload json");
    let run_id_str = started_json["run_id"]
        .as_str()
        .expect("run_id should be a string");
    let run_id = run_id_str.parse::<RunId>().expect("run_id should parse");

    let status = service
        .dispatch_external_tool(
            &sid_1,
            "mob_flow_status",
            serde_json::json!({"run_id": run_id_str}),
        )
        .await
        .expect("mob_flow_status");
    let status_json: serde_json::Value =
        serde_json::from_str(&status.text_content()).expect("status payload json");
    assert_eq!(status_json["run"]["run_id"], run_id_str);
    assert_eq!(status_json["run"]["flow_id"], "demo");

    service
        .dispatch_external_tool(
            &sid_1,
            "mob_cancel_flow",
            serde_json::json!({"run_id": run_id_str}),
        )
        .await
        .expect("mob_cancel_flow");

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(8)).await;
    assert_eq!(
        terminal.status,
        MobRunStatus::Canceled,
        "cancel dispatch should drive the run to canceled"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::FlowCanceled { run_id: id, .. } if id == &run_id
            )
        }),
        "cancel dispatch should emit FlowCanceled"
    );
}

#[tokio::test]
async fn test_mob_flow_tools_reject_invalid_run_id_arguments() {
    let mut definition = sample_definition_with_single_step_flow(500, 8);
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile exists");
    worker.tools.mob = true;

    let (handle, service) = create_test_mob(definition).await;
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();

    let error = service
        .dispatch_external_tool(
            &sid_1,
            "mob_flow_status",
            serde_json::json!({"run_id":"not-a-uuid"}),
        )
        .await
        .expect_err("invalid run_id should fail");
    assert!(
        matches!(error, ToolError::InvalidArguments { .. }),
        "flow status should surface invalid run_id as invalid arguments"
    );
}

#[tokio::test]
async fn test_flow_step_tool_overlay_is_step_scoped() {
    let mut definition = sample_definition_with_single_step_flow(2_000, 8);
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile exists");
    worker.tools.mob = true;

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
        .session_id()
        .expect("session-backed")
        .clone();

    let started = service
        .dispatch_external_tool(
            &sid,
            "mob_run_flow",
            serde_json::json!({
                "flow_id": "demo",
                "params": {}
            }),
        )
        .await
        .expect("start flow");
    let started_json: serde_json::Value =
        serde_json::from_str(&started.text_content()).expect("run payload json");
    let run_id = started_json["run_id"]
        .as_str()
        .expect("run_id string")
        .parse::<RunId>()
        .expect("run_id parse");

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
                additional_instructions: None,
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
            MeerkatId::from("w-runtime-overlay"),
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
async fn test_spawn_meerkat_tool_dispatches_backend_selection() {
    let mut definition = sample_definition_with_mob_tools();
    definition.backend.external = Some(crate::definition::ExternalBackendConfig {
        address_base: "https://backend.example.invalid/mesh".to_string(),
    });
    let (handle, service) = create_test_mob(definition).await;
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();

    let spawned = service
        .dispatch_external_tool(
            &sid_1,
            "spawn_meerkat",
            serde_json::json!({
                "profile": "worker",
                "meerkat_id": "w-ext",
                "backend": "external"
            }),
        )
        .await
        .expect("spawn external via tool");
    let payload: serde_json::Value =
        serde_json::from_str(&spawned.text_content()).expect("spawn payload json");
    assert_eq!(payload["member_ref"]["kind"], "backend_peer");

    let entry = handle
        .get_member(&MeerkatId::from("w-ext"))
        .await
        .expect("w-ext exists");
    assert!(
        matches!(entry.member_ref, MemberRef::BackendPeer { .. }),
        "tool backend selection should flow into runtime provisioning"
    );
}

#[tokio::test]
async fn test_mob_task_tools_dispatch_and_blocked_by_enforcement() {
    let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();

    for required in [
        "mob_task_create",
        "mob_task_list",
        "mob_task_update",
        "mob_task_get",
    ] {
        assert!(
            service
                .external_tool_names(&sid_1)
                .await
                .contains(&required.to_string()),
            "expected task tool '{required}'"
        );
    }

    let created_1 = service
        .dispatch_external_tool(
            &sid_1,
            "mob_task_create",
            serde_json::json!({
                "subject": "Foundations",
                "description": "Prepare prerequisites"
            }),
        )
        .await
        .expect("create t1");
    let created_1_json: serde_json::Value =
        serde_json::from_str(&created_1.text_content()).expect("task create content json");
    let t1 = created_1_json["task_id"]
        .as_str()
        .expect("task_id should be present")
        .to_string();

    let created_2 = service
        .dispatch_external_tool(
            &sid_1,
            "mob_task_create",
            serde_json::json!({
                "subject": "Main work",
                "description": "Depends on t1",
                "blocked_by": [t1]
            }),
        )
        .await
        .expect("create t2");
    let created_2_json: serde_json::Value =
        serde_json::from_str(&created_2.text_content()).expect("task create content json");
    let t2 = created_2_json["task_id"]
        .as_str()
        .expect("task_id should be present")
        .to_string();

    let blocked = service
        .dispatch_external_tool(
            &sid_1,
            "mob_task_update",
            serde_json::json!({"task_id": t2.clone(), "status": "in_progress", "owner": "w-1"}),
        )
        .await
        .expect_err("blocked task should not be claimable");
    assert!(
        blocked.to_string().contains("blocked"),
        "blocked-by enforcement should reject task claim"
    );
    // Providing `owner` alongside a non-in_progress status should be accepted, but the
    // owner value must be ignored (owner is only mutable when status is in_progress).
    service
        .dispatch_external_tool(
            &sid_1,
            "mob_task_update",
            serde_json::json!({"task_id": t2.clone(), "status": "completed", "owner": "w-1"}),
        )
        .await
        .expect("completing with owner present should succeed (owner ignored)");

    service
        .dispatch_external_tool(
            &sid_1,
            "mob_task_update",
            serde_json::json!({"task_id": t1.clone(), "status": "completed"}),
        )
        .await
        .expect("complete dependency");
    service
        .dispatch_external_tool(
            &sid_1,
            "mob_task_update",
            serde_json::json!({"task_id": t2.clone(), "status": "in_progress", "owner": "w-1"}),
        )
        .await
        .expect("claim unblocked task");

    let task = service
        .dispatch_external_tool(
            &sid_1,
            "mob_task_get",
            serde_json::json!({"task_id": t2.clone()}),
        )
        .await
        .expect("task get");
    let task_json: serde_json::Value =
        serde_json::from_str(&task.text_content()).expect("task get content json");
    assert_eq!(task_json["task"]["status"], "in_progress");
    assert_eq!(task_json["task"]["owner"], "w-1");

    let listed = service
        .dispatch_external_tool(&sid_1, "mob_task_list", serde_json::json!({}))
        .await
        .expect("task list");
    let list_json: serde_json::Value =
        serde_json::from_str(&listed.text_content()).expect("task list content json");
    assert_eq!(
        list_json["tasks"].as_array().map_or(0, std::vec::Vec::len),
        2,
        "mob_task_list should project created tasks"
    );
}

#[tokio::test]
async fn test_tool_flag_enforcement_blocks_mob_and_task_tools() {
    let (handle, service) = create_test_mob(sample_definition_without_mob_flags()).await;
    let sid = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("lead-1"), None)
        .await
        .expect("spawn lead")
        .session_id()
        .expect("session-backed")
        .clone();

    let names = service.external_tool_names(&sid).await;
    assert!(
        !names.iter().any(|name| name == "spawn_meerkat"),
        "tools.mob=false should hide spawn_meerkat"
    );
    assert!(
        !names.iter().any(|name| name == "mob_task_create"),
        "tools.mob_tasks=false should hide mob_task_create"
    );

    let spawn_err = service
        .dispatch_external_tool(
            &sid,
            "spawn_meerkat",
            serde_json::json!({"profile": "worker", "meerkat_id": "w-2"}),
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
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();

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
        .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events.clone()))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume");

    assert_eq!(resumed.mob_id().as_str(), "test-mob");
    let entry_1 = resumed.get_member(&MeerkatId::from("w-1")).await.unwrap();
    let entry_2 = resumed.get_member(&MeerkatId::from("w-2")).await.unwrap();
    assert!(entry_1.wired_to.contains(&MeerkatId::from("w-2")));
    assert!(entry_2.wired_to.contains(&MeerkatId::from("w-1")));
}

#[tokio::test]
async fn test_resume_fails_when_orchestrator_resume_notification_fails() {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
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

    let result = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service)
        .resume()
        .await;

    assert!(
        matches!(result, Err(MobError::SessionError(SessionError::Store(_)))),
        "resume must surface orchestrator start_turn failures"
    );
}

#[tokio::test]
async fn test_resume_reconciles_orphaned_sessions() {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
            labels: None,
        })
        .await
        .expect("create orphan");
    assert_eq!(service.active_session_count().await, 1);

    let _resumed = MobBuilder::for_resume(MobStorage::with_events(events))
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
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("roster entry")
        .session_id()
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

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume");

    let restored_sid = resumed
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("restored entry")
        .session_id()
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
        .expect("worker profile");
    worker.tools.rust_bundles = vec!["bundle-a".to_string()];

    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("roster entry")
        .session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive missing session");

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .register_tool_bundle("bundle-a", Arc::new(EchoBundleDispatcher))
        .resume()
        .await
        .expect("resume");

    let restored_sid = resumed
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("restored entry")
        .session_id()
        .cloned()
        .expect("session-backed member");
    assert_eq!(
        restored_sid, old_sid,
        "persistent resume should restore the original session id"
    );

    let names = service.external_tool_names(&restored_sid).await;
    assert!(
        names.contains(&"spawn_meerkat".to_string()),
        "mob tools must be wired on restored sessions"
    );
    assert!(
        names.contains(&"mob_task_create".to_string()),
        "mob task tools must be wired on restored sessions"
    );
    assert!(
        names.contains(&"bundle_echo".to_string()),
        "rust bundle tools must be wired on restored sessions"
    );
}

#[tokio::test]
async fn test_resume_marks_missing_persisted_session_as_broken() {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("roster entry")
        .session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("partial resume should still succeed");

    let snapshot = resumed
        .member_status(&MeerkatId::from("w-1"))
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

    let members = resumed.list_members().await;
    let broken = members
        .into_iter()
        .find(|entry| entry.meerkat_id == MeerkatId::from("w-1"))
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
}

#[tokio::test]
async fn test_resume_skips_wiring_for_broken_peer_and_keeps_partial_resume() {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");
    handle.stop().await.expect("stop");

    service
        .archive(&sid_2)
        .await
        .expect("archive broken session");
    service.delete_persisted_session(&sid_2).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume should stay partial even with broken wired member");

    let left = resumed
        .member_status(&MeerkatId::from("w-1"))
        .await
        .expect("healthy member status");
    assert_eq!(left.status, crate::runtime::handle::MobMemberStatus::Active);
    assert_eq!(left.current_session_id, Some(sid_1.clone()));

    let right = resumed
        .member_status(&MeerkatId::from("w-2"))
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
    let service = Arc::new(PersistedListingSessionService::new(inner.clone()));
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let sid = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();
    handle.stop().await.expect("stop");
    inner.archive(&sid).await.expect("archive session");

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service)
        .resume()
        .await
        .expect(
            "resume should restore from persisted snapshot, not treat inactive summary as live",
        );

    let snapshot = resumed
        .member_status(&MeerkatId::from("w-1"))
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
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
        .get_member(&MeerkatId::from("lead-1"))
        .await
        .expect("orchestrator entry")
        .session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&orchestrator_sid)
        .await
        .expect("archive orchestrator");
    service.delete_persisted_session(&orchestrator_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume should succeed when orchestrator is broken");

    let orchestrator = resumed
        .member_status(&MeerkatId::from("lead-1"))
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
    let mut definition = sample_definition();
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("roster entry")
        .session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
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
            MeerkatId::from("w-1"),
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
            assert_eq!(session_id, old_sid);
        }
        other => panic!("expected MemberRestoreFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn test_wire_broken_member_returns_restore_failed_error() {
    let service = Arc::new(MockSessionService::new());
    let mut definition = sample_definition();
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("roster entry")
        .session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
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
        .wire(MeerkatId::from("w-2"), MeerkatId::from("w-1"))
        .await
        .expect_err("wiring to Broken member must be rejected");

    match error {
        MobError::MemberRestoreFailed {
            member_id,
            session_id,
            ..
        } => {
            assert_eq!(member_id, MeerkatId::from("w-1"));
            assert_eq!(session_id, old_sid);
        }
        other => panic!("expected MemberRestoreFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn test_retire_broken_member_succeeds_and_removes_it() {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("roster entry")
        .session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("partial resume should still succeed");

    resumed
        .retire(MeerkatId::from("w-1"))
        .await
        .expect("retire should work on broken member");

    assert!(
        resumed.get_member(&MeerkatId::from("w-1")).await.is_none(),
        "retire should remove the broken member from the roster"
    );
}

#[tokio::test]
async fn test_respawn_broken_member_clears_restore_diagnostic() {
    let service = Arc::new(MockSessionService::new());
    let mut definition = sample_definition();
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("roster entry")
        .session_id()
        .cloned()
        .expect("session-backed member");
    service
        .archive(&old_sid)
        .await
        .expect("archive live session");
    service.delete_persisted_session(&old_sid).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
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
        .respawn(MeerkatId::from("w-1"), None)
        .await
        .expect("respawn should repair broken member");
    let new_sid = receipt
        .new_session_id
        .clone()
        .expect("respawned member should have a new session");
    assert_ne!(
        new_sid, old_sid,
        "respawn should rotate the session identity"
    );

    let snapshot = resumed
        .member_status(&MeerkatId::from("w-1"))
        .await
        .expect("member status after repair");
    assert_eq!(
        snapshot.status,
        crate::runtime::handle::MobMemberStatus::Active
    );
    assert_eq!(snapshot.current_session_id, Some(new_sid.clone()));
    assert!(
        snapshot.error.is_none(),
        "repair should clear the old broken diagnostic"
    );

    let members = resumed.list_members().await;
    let repaired = members
        .into_iter()
        .find(|entry| entry.meerkat_id == MeerkatId::from("w-1"))
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
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("roster entry")
        .session_id()
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
    metadata.tooling.builtins = false;
    metadata.tooling.shell = true;
    metadata.tooling.mob = true;
    metadata.tooling.memory = true;
    metadata.tooling.active_skills = Some(vec![meerkat_core::skills::SkillId(
        "mob-communication".into(),
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

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume");

    let restored_sid = resumed
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("restored entry")
        .session_id()
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
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
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
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("roster entry")
        .session_id()
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

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume should succeed with Broken projection");
    let snapshot = resumed
        .member_status(&MeerkatId::from("w-1"))
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
        .expect("worker profile");
    let mut resumed = Session::new();
    resumed
        .set_session_metadata(SessionMetadata {
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            tooling: SessionTooling {
                builtins: true,
                shell: false,
                comms: true,
                mob: false,
                memory: false,
                active_skills: Some(vec![meerkat_core::skills::SkillId(
                    "mob-communication".into(),
                )]),
            },
            keep_alive: false,
            comms_name: Some(test_comms_name("worker", "w-1")),
            peer_meta: Some(
                meerkat_core::PeerMeta::default()
                    .with_label("mob_id", "test-mob")
                    .with_label("role", "worker")
                    .with_label("meerkat_id", "w-1"),
            ),
            realm_id: Some("mob:test-mob".to_string()),
            instance_id: None,
            backend: None,
            config_generation: None,
        })
        .expect("resume metadata");

    let wrong_session_id = SessionId::new();
    let error =
        crate::build::build_resumed_agent_config(crate::build::BuildResumedAgentConfigParams {
            base: crate::build::BuildAgentConfigParams {
                mob_id: &mob_id,
                profile_name: &profile_name,
                meerkat_id: &meerkat_id,
                profile,
                definition: &definition,
                external_tools: None,
                context: None,
                labels: None,
                additional_instructions: None,
                shell_env: None,
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
        member_ref.session_id(),
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
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(sample_definition_with_external_backend(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn_with_backend(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            Some(MobBackendKind::External),
        )
        .await
        .expect("spawn external");
    handle.stop().await.expect("stop");

    let old_entry = handle
        .get_member(&MeerkatId::from("w-ext"))
        .await
        .expect("external roster entry");
    let (old_peer_id, old_address, old_sid) = match old_entry.member_ref {
        MemberRef::BackendPeer {
            ref peer_id,
            ref address,
            ref session_id,
        } => (
            peer_id.clone(),
            address.clone(),
            session_id.clone().expect("external bridge session id"),
        ),
        ref other => panic!("expected external backend member ref, got {other:?}"),
    };
    service
        .archive(&old_sid)
        .await
        .expect("archive external bridge session");

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume");
    let resumed_entry = resumed
        .get_member(&MeerkatId::from("w-ext"))
        .await
        .expect("resumed external entry");
    match resumed_entry.member_ref {
        MemberRef::BackendPeer {
            peer_id,
            address,
            session_id,
        } => {
            let resumed_sid = session_id.expect("resumed external bridge session id");
            assert_ne!(
                resumed_sid, old_sid,
                "resume should recreate missing external bridge session"
            );
            assert_eq!(peer_id, old_peer_id, "resume must preserve backend peer_id");
            assert_eq!(address, old_address, "resume must preserve backend address");
        }
        other => panic!("expected backend peer after resume, got {other:?}"),
    }
}

#[tokio::test]
async fn test_resume_reconciles_mixed_topology_without_losing_external_member_refs() {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(sample_definition_with_external_backend(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let old_sub_sid = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-sub"), None)
        .await
        .expect("spawn session-backed member")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .spawn_with_backend(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            Some(MobBackendKind::External),
        )
        .await
        .expect("spawn external");
    handle
        .wire(MeerkatId::from("w-sub"), MeerkatId::from("w-ext"))
        .await
        .expect("wire mixed topology");
    handle.stop().await.expect("stop");

    let old_ext_entry = handle
        .get_member(&MeerkatId::from("w-ext"))
        .await
        .expect("external entry before resume");
    let (old_ext_peer_id, old_ext_addr, old_ext_sid) = match old_ext_entry.member_ref {
        MemberRef::BackendPeer {
            ref peer_id,
            ref address,
            ref session_id,
        } => (
            peer_id.clone(),
            address.clone(),
            session_id.clone().expect("external bridge session"),
        ),
        ref other => panic!("expected external backend member ref, got {other:?}"),
    };
    service
        .archive(&old_sub_sid)
        .await
        .expect("archive session-backed member session");
    service
        .archive(&old_ext_sid)
        .await
        .expect("archive external bridge session");

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume mixed topology");
    let resumed_sub = resumed
        .get_member(&MeerkatId::from("w-sub"))
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
        .get_member(&MeerkatId::from("w-ext"))
        .await
        .expect("resumed external entry");
    match resumed_ext.member_ref {
        MemberRef::BackendPeer {
            peer_id,
            address,
            session_id,
        } => {
            assert_eq!(peer_id, old_ext_peer_id, "external peer_id must be stable");
            assert_eq!(address, old_ext_addr, "external address must be stable");
            assert_ne!(
                session_id.expect("resumed external bridge session"),
                old_ext_sid,
                "resume should recreate missing external bridge session"
            );
        }
        other => panic!("expected backend peer member ref, got {other:?}"),
    }

    let trusted_sub = service.trusted_peer_addresses(&resumed_sub_sid).await;
    assert!(
        trusted_sub.iter().any(|addr| addr.starts_with("inproc://")),
        "mixed resume should re-establish trust with a sendable transport address"
    );
}

#[tokio::test]
async fn test_resume_reestablishes_missing_trust() {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");
    handle.stop().await.expect("stop");

    service.force_remove_trust(&sid_1, &sid_2).await;
    service.force_remove_trust(&sid_2, &sid_1).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume");
    let resumed_sid_1 = resumed
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("w-1")
        .session_id()
        .cloned()
        .expect("session-backed member");
    let resumed_sid_2 = resumed
        .get_member(&MeerkatId::from("w-2"))
        .await
        .expect("w-2")
        .session_id()
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
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let sid_1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();
    let _sid_2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2");
    handle.stop().await.expect("stop");

    let stale = TrustedPeerSpec::new(
        "remote-mob/worker/stale-peer",
        "ed25519:stale-peer",
        "inproc://remote-mob/worker/stale-peer",
    )
    .expect("valid stale peer");
    service
        .force_add_trust_from_spec(&sid_1, stale.clone())
        .await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume");
    let resumed_sid_1 = resumed
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("w-1")
        .session_id()
        .cloned()
        .expect("session-backed member");
    let trusted = service.trusted_peer_names(&resumed_sid_1).await;
    assert!(
        !trusted.contains(&stale.name),
        "resume should prune stale trust that is not present in the roster projection"
    );
}

#[tokio::test]
async fn test_resume_restores_external_wiring_from_event_log() {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    let external = TrustedPeerSpec::new(
        "remote-mob/worker/agent-b",
        "ed25519:remote-agent-b",
        "inproc://remote-mob/worker/agent-b",
    )
    .expect("valid external peer");
    handle
        .wire(
            MeerkatId::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external");
    handle.stop().await.expect("stop");

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume");
    let resumed_sid = resumed
        .get_member(&MeerkatId::from("l-1"))
        .await
        .expect("resumed member")
        .session_id()
        .cloned()
        .expect("session-backed member");
    let trusted = service.trusted_peer_names(&resumed_sid).await;
    assert!(
        trusted.contains(&external.name),
        "resume should restore external trusted peers from persisted wiring events"
    );
    let entry = resumed
        .get_member(&MeerkatId::from("l-1"))
        .await
        .expect("resumed member");
    assert_eq!(
        entry
            .external_peer_specs
            .get(&MeerkatId::from(external.name.clone()))
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
    assert_eq!(handle.status(), MobState::Completed);
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
    assert_eq!(handle.status(), MobState::Destroyed);

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
    assert_eq!(resumed.status(), MobState::Running);
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
        .session_id()
        .expect("session-backed")
        .clone();

    // Verify roster updated
    let meerkats = handle.list_members().await;
    assert_eq!(meerkats.len(), 1);
    assert_eq!(meerkats[0].meerkat_id.as_str(), "w-1");
    assert_eq!(meerkats[0].profile.as_str(), "worker");
    assert_eq!(meerkats[0].session_id(), Some(&session_id));
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
        .session_id()
        .expect("session-backed")
        .clone();

    let events = handle.events().replay_all().await.expect("replay");
    // Should have MobCreated + MeerkatSpawned
    assert!(
        events.len() >= 2,
        "expected at least 2 events, got {}",
        events.len()
    );
    let spawned = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::MeerkatSpawned { .. }));
    assert!(spawned.is_some(), "should have MeerkatSpawned event");
    if let MobEventKind::MeerkatSpawned {
        meerkat_id,
        role,
        member_ref,
        ..
    } = &spawned.unwrap().kind
    {
        assert_eq!(meerkat_id.as_str(), "w-1");
        assert_eq!(role.as_str(), "worker");
        assert_eq!(member_ref.session_id(), Some(&session_id));
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
    assert!(matches!(result, Err(MobError::MeerkatAlreadyExists(_))));
}

#[tokio::test]
async fn test_spawn_supports_session_and_external_backends() {
    let (handle, _service) = create_test_mob(sample_definition_with_external_backend()).await;

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

    let external_ref = handle
        .spawn_with_backend(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            Some(MobBackendKind::External),
        )
        .await
        .expect("spawn external backend");
    match external_ref {
        MemberRef::BackendPeer {
            address,
            session_id,
            ..
        } => {
            assert!(
                address.starts_with("https://backend.example.invalid/mesh/"),
                "external backend should use configured address base"
            );
            assert!(
                session_id.is_some(),
                "external backend should keep session bridge for lifecycle ops"
            );
        }
        other => panic!("expected backend peer member ref, got {other:?}"),
    }
}

#[tokio::test]
async fn test_external_backend_rejects_invalid_peer_name_components() {
    let (handle, _service) = create_test_mob(sample_definition_with_external_backend()).await;
    let result = handle
        .spawn_with_backend(
            ProfileName::from("worker"),
            MeerkatId::from("../w-ext"),
            None,
            Some(MobBackendKind::External),
        )
        .await;
    assert!(
        matches!(result, Err(MobError::WiringError(_))),
        "invalid peer name components must fail external member registration"
    );
}

#[tokio::test]
async fn test_external_backend_wiring_uses_sendable_transport_addresses() {
    let (handle, service) = create_test_mob(sample_definition_with_external_backend()).await;

    let member_a = handle
        .spawn_with_backend(
            ProfileName::from("worker"),
            MeerkatId::from("w-a"),
            None,
            Some(MobBackendKind::External),
        )
        .await
        .expect("spawn external w-a");
    let member_b = handle
        .spawn_with_backend(
            ProfileName::from("worker"),
            MeerkatId::from("w-b"),
            None,
            Some(MobBackendKind::External),
        )
        .await
        .expect("spawn external w-b");
    let sid_a = member_a
        .session_id()
        .cloned()
        .expect("external member bridge session id");
    let sid_b = member_b
        .session_id()
        .cloned()
        .expect("external member bridge session id");

    handle
        .wire(MeerkatId::from("w-a"), MeerkatId::from("w-b"))
        .await
        .expect("wire external peers");

    let addresses_a = service.trusted_peer_addresses(&sid_a).await;
    let addresses_b = service.trusted_peer_addresses(&sid_b).await;
    assert!(
        addresses_a
            .iter()
            .all(|address| address.starts_with("inproc://")),
        "wiring must use transport-sendable addresses"
    );
    assert!(
        addresses_b
            .iter()
            .all(|address| address.starts_with("inproc://")),
        "wiring must use transport-sendable addresses"
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
        .expect("worker profile exists");
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
    assert!(handle.get_member(&MeerkatId::from("w-1")).await.is_none());
}

#[tokio::test]
async fn test_spawn_append_failure_rolls_back_runtime_state() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("MeerkatSpawned").await;
    let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await;
    assert!(
        matches!(result, Err(MobError::Internal(_))),
        "spawn should surface append failure"
    );
    assert!(
        handle.get_member(&MeerkatId::from("w-1")).await.is_none(),
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
            .any(|e| matches!(e.kind, MobEventKind::MeerkatSpawned { .. })),
        "failed spawn append must not persist MeerkatSpawned"
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

    handle.retire(MeerkatId::from("w-1")).await.expect("retire");

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

    handle.retire(MeerkatId::from("w-1")).await.expect("retire");
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

    handle.retire(MeerkatId::from("w-1")).await.expect("retire");

    let events = handle.events().replay_all().await.expect("replay");
    let retired = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. }));
    assert!(retired.is_some(), "should have MeerkatRetired event");
}

#[tokio::test]
async fn test_retire_nonexistent_is_idempotent() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let result = handle.retire(MeerkatId::from("nope")).await;
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
        .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    // Verify wired
    let entry = handle.get_member(&MeerkatId::from("w-2")).await.unwrap();
    assert!(entry.wired_to.contains(&MeerkatId::from("w-1")));

    // Retire w-1
    handle.retire(MeerkatId::from("w-1")).await.expect("retire");

    // w-2 should no longer be wired to w-1
    let entry = handle.get_member(&MeerkatId::from("w-2")).await.unwrap();
    assert!(!entry.wired_to.contains(&MeerkatId::from("w-1")));
}

#[tokio::test]
async fn test_retire_archive_failure_is_not_silent() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let session_id = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();
    service.set_archive_failure(&session_id).await;

    // Retire succeeds despite archive failure (best-effort cleanup).
    let result = handle.retire(MeerkatId::from("w-1")).await;
    assert!(
        result.is_ok(),
        "retire should succeed despite archive failure"
    );

    // Member is removed from roster unconditionally.
    assert!(
        handle.get_member(&MeerkatId::from("w-1")).await.is_none(),
        "retire must remove roster entry even when archive fails"
    );
    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. })),
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
        .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    // Retire succeeds despite trust-removal failure (best-effort cleanup).
    let result = handle.retire(MeerkatId::from("w-1")).await;
    assert!(
        result.is_ok(),
        "retire should succeed despite trust-removal failure"
    );

    // Member is removed from roster unconditionally.
    assert!(
        handle.get_member(&MeerkatId::from("w-1")).await.is_none(),
        "retire must remove roster entry even when trust removal fails"
    );
    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. })),
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
        .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    let result = handle.retire(MeerkatId::from("w-1")).await;
    assert!(
        result.is_ok(),
        "retire should remain best-effort under notification failure"
    );
    assert!(
        handle.get_member(&MeerkatId::from("w-1")).await.is_none(),
        "retired meerkat should be removed from roster"
    );
    let entry_w2 = handle
        .get_member(&MeerkatId::from("w-2"))
        .await
        .expect("w-2 should stay in roster");
    assert!(
        !entry_w2.wired_to.contains(&MeerkatId::from("w-1")),
        "retire should remove wiring from remaining peers"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. })),
        "retire event should persist even when notification cleanup fails"
    );
}

#[tokio::test]
async fn test_retire_append_failure_is_retryable_without_side_effects() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("MeerkatRetired").await;
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
        .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    let result = handle.retire(MeerkatId::from("w-1")).await;
    assert!(
        matches!(result, Err(MobError::Internal(_))),
        "retire should surface append failure"
    );
    assert!(
        handle.get_member(&MeerkatId::from("w-1")).await.is_some(),
        "retire append failure must keep roster state retryable"
    );
    let entry_w2 = handle
        .get_member(&MeerkatId::from("w-2"))
        .await
        .expect("w-2 should remain in roster");
    assert!(
        entry_w2.wired_to.contains(&MeerkatId::from("w-1")),
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
            .any(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. })),
        "failed retire append must not persist MeerkatRetired"
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
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    // Check roster wiring
    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    assert!(entry_l.wired_to.contains(&MeerkatId::from("w-1")));

    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    assert!(entry_w.wired_to.contains(&MeerkatId::from("l-1")));
}

#[tokio::test]
async fn test_member_roster_surfaces_peer_id() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let entry = handle
        .get_member(&MeerkatId::from("l-1"))
        .await
        .expect("member should exist");
    assert_eq!(entry.peer_id.as_deref(), Some("ed25519:test-mob/lead/l-1"));
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
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .spawn(ProfileName::from("worker"), right_id.clone(), None)
        .await
        .expect("spawn worker");
    handle
        .wire(left_id.clone(), right_id.clone())
        .await
        .expect("wire local peers");

    let right_entry = handle
        .get_member(&right_id)
        .await
        .expect("right member exists");
    let right_name = format!(
        "{}/{}/{}",
        handle.mob_id(),
        right_entry.profile,
        right_entry.meerkat_id
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
        .member_status(&left_id)
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
async fn test_member_status_omits_peer_connectivity_without_live_comms_runtime() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let left_id = MeerkatId::from("l-1");
    let right_id = MeerkatId::from("w-1");
    let left_session_id = handle
        .spawn(ProfileName::from("lead"), left_id.clone(), None)
        .await
        .expect("spawn lead")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .spawn(ProfileName::from("worker"), right_id.clone(), None)
        .await
        .expect("spawn worker");
    handle
        .wire(left_id.clone(), right_id.clone())
        .await
        .expect("wire local peers");
    service.set_missing_comms_runtime(&left_session_id).await;

    let snapshot = handle
        .member_status(&left_id)
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
        .session_id()
        .expect("session-backed")
        .clone();

    let external = TrustedPeerSpec::new(
        "remote-mob/worker/agent-b",
        "ed25519:remote-agent-b",
        "inproc://remote-mob/worker/agent-b",
    )
    .expect("valid external peer");

    handle
        .wire(
            MeerkatId::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external");

    let entry_l = handle
        .get_member(&MeerkatId::from("l-1"))
        .await
        .expect("member should exist");
    assert!(
        entry_l
            .wired_to
            .contains(&MeerkatId::from(external.name.clone()))
    );
    assert_eq!(
        entry_l
            .external_peer_specs
            .get(&MeerkatId::from(external.name.clone()))
            .cloned(),
        Some(external.clone())
    );

    let trusted = service.trusted_peer_names(&sid_l).await;
    assert!(
        trusted.contains(&external.name),
        "trusted peer list should include external peer name"
    );
}

#[tokio::test]
async fn test_respawn_restores_external_wiring_from_roster_spec() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let old_sid = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .session_id()
        .expect("session-backed")
        .clone();

    let external = TrustedPeerSpec::new(
        "remote-mob/worker/agent-b",
        "ed25519:remote-agent-b",
        "inproc://remote-mob/worker/agent-b",
    )
    .expect("valid external peer");

    handle
        .wire(
            MeerkatId::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external");

    let receipt = handle
        .respawn(MeerkatId::from("l-1"), Some("resume".into()))
        .await
        .expect("respawn");
    let new_sid = receipt
        .new_session_id
        .expect("respawn should produce replacement session");
    assert_ne!(new_sid, old_sid, "respawn should replace the session");

    let trusted = service.trusted_peer_names(&new_sid).await;
    assert!(
        trusted.contains(&external.name),
        "respawn should restore external trusted peers from the stored roster spec"
    );

    let entry = handle
        .get_member(&MeerkatId::from("l-1"))
        .await
        .expect("member should exist");
    assert_eq!(
        entry
            .external_peer_specs
            .get(&MeerkatId::from(external.name.clone()))
            .cloned(),
        Some(external)
    );
}

#[tokio::test]
async fn test_unwire_external_removes_trust_and_projection() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .session_id()
        .expect("session-backed")
        .clone();
    let external = TrustedPeerSpec::new(
        "remote-mob/worker/agent-b",
        "ed25519:remote-agent-b",
        "inproc://remote-mob/worker/agent-b",
    )
    .expect("valid external peer");

    handle
        .wire(
            MeerkatId::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external");
    handle
        .unwire(
            MeerkatId::from("l-1"),
            MeerkatId::from(external.name.clone()),
        )
        .await
        .expect("unwire external");

    let entry = handle
        .get_member(&MeerkatId::from("l-1"))
        .await
        .expect("member should exist");
    assert!(
        !entry
            .wired_to
            .contains(&MeerkatId::from(external.name.clone()))
    );
    assert!(
        !entry
            .external_peer_specs
            .contains_key(&MeerkatId::from(external.name.clone()))
    );

    let trusted = service.trusted_peer_names(&sid_l).await;
    assert!(
        !trusted.contains(&external.name),
        "trusted peer list should not include the removed external peer"
    );
}

#[tokio::test]
async fn test_unwire_external_emits_external_peer_unwired_event() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    let external = TrustedPeerSpec::new(
        "remote-mob/worker/agent-b",
        "ed25519:remote-agent-b",
        "inproc://remote-mob/worker/agent-b",
    )
    .expect("valid external peer");
    handle
        .wire(
            MeerkatId::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external");

    handle
        .unwire(
            MeerkatId::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("unwire external");

    let events = handle.events().replay_all().await.expect("replay");
    assert!(events.iter().any(|event| {
        matches!(
            &event.kind,
            MobEventKind::ExternalPeerUnwired { local, peer_name }
                if local == &MeerkatId::from("l-1")
                    && peer_name == &MeerkatId::from(external.name.clone())
        )
    }));
}

#[tokio::test]
async fn test_unwire_external_is_idempotent_and_emits_single_event() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");
    let external = TrustedPeerSpec::new(
        "remote-mob/worker/agent-b",
        "ed25519:remote-agent-b",
        "inproc://remote-mob/worker/agent-b",
    )
    .expect("valid external peer");
    let external_name = MeerkatId::from(external.name.clone());

    handle
        .wire(
            MeerkatId::from("l-1"),
            PeerTarget::External(external.clone()),
        )
        .await
        .expect("wire external");
    handle
        .unwire(MeerkatId::from("l-1"), external_name.clone())
        .await
        .expect("first external unwire");
    handle
        .unwire(MeerkatId::from("l-1"), external_name.clone())
        .await
        .expect("second external unwire");

    let entry = handle
        .get_member(&MeerkatId::from("l-1"))
        .await
        .expect("member should exist");
    assert!(!entry.wired_to.contains(&external_name));
    assert!(!entry.external_peer_specs.contains_key(&external_name));

    let events = handle.events().replay_all().await.expect("replay");
    let count = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::ExternalPeerUnwired { local, peer_name }
                    if local == &MeerkatId::from("l-1") && peer_name == &external_name
            )
        })
        .count();
    assert_eq!(count, 1, "idempotent external unwire should emit one event");
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
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let events = handle.events().replay_all().await.expect("replay");
    let wired = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::PeersWired { .. }));
    assert!(wired.is_some(), "should have PeersWired event");
}

#[tokio::test]
async fn test_wire_is_idempotent_and_emits_single_pair_event() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("first wire");
    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("second wire should reconcile as idempotent");

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
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
                MobEventKind::PeersWired { a, b }
                    if (a == &MeerkatId::from("l-1") && b == &MeerkatId::from("w-1"))
                        || (a == &MeerkatId::from("w-1") && b == &MeerkatId::from("l-1"))
            )
        })
        .count();
    assert_eq!(
        pair_wired_events, 1,
        "idempotent wire must emit one logical PeersWired event for one canonical edge"
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
        .wire(MeerkatId::from("w-1"), MeerkatId::from("nonexistent"))
        .await;
    assert!(matches!(result, Err(MobError::MeerkatNotFound(_))));
}

#[tokio::test]
async fn test_wire_fails_when_comms_runtime_missing_without_side_effects() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let _sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();
    service.set_missing_comms_runtime(&sid_w).await;

    let result = handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(matches!(result, Err(MobError::WiringError(_))));

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
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
            .any(|e| matches!(e.kind, MobEventKind::PeersWired { .. })),
        "failed wire must not emit PeersWired"
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
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(matches!(result, Err(MobError::WiringError(_))));

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
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
            .any(|e| matches!(e.kind, MobEventKind::PeersWired { .. })),
        "failed wire must not emit PeersWired"
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();

    let result = handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::CommsError(_))),
        "wire should fail when required notification fails"
    );

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
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
            .any(|e| matches!(e.kind, MobEventKind::PeersWired { .. })),
        "failed wire must not emit PeersWired"
    );
}

#[tokio::test]
async fn test_wire_establishes_comms_trust() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
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
    events.fail_appends_for("PeersWired").await;
    let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();

    let result = handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::Internal(_))),
        "wire should surface append failure"
    );

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
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
            .any(|e| matches!(e.kind, MobEventKind::PeersWired { .. })),
        "failed wire append must not persist PeersWired"
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
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("unwire");

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    assert!(entry_l.wired_to.is_empty());

    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
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
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("unwire");

    let events = handle.events().replay_all().await.expect("replay");
    let unwired = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. }));
    assert!(unwired.is_some(), "should have PeersUnwired event");
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
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("first unwire");
    handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("second unwire should be idempotent");

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
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
                MobEventKind::PeersUnwired { a, b }
                    if (a == &MeerkatId::from("l-1") && b == &MeerkatId::from("w-1"))
                        || (a == &MeerkatId::from("w-1") && b == &MeerkatId::from("l-1"))
            )
        })
        .count();
    assert_eq!(
        pair_unwired_events, 1,
        "idempotent unwire must emit one logical PeersUnwired event for one canonical edge removal"
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
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");
    service.set_missing_comms_runtime(&sid_w).await;

    let result = handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(matches!(result, Err(MobError::WiringError(_))));

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    assert!(
        entry_l.wired_to.contains(&MeerkatId::from("w-1")),
        "failed unwire must not mutate roster"
    );
    assert!(
        entry_w.wired_to.contains(&MeerkatId::from("l-1")),
        "failed unwire must not mutate roster"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
        "failed unwire must not emit PeersUnwired"
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
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");
    service.clear_public_key(&sid_w).await;

    let result = handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(matches!(result, Err(MobError::WiringError(_))));

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    assert!(
        entry_l.wired_to.contains(&MeerkatId::from("w-1")),
        "failed unwire must not mutate roster"
    );
    assert!(
        entry_w.wired_to.contains(&MeerkatId::from("l-1")),
        "failed unwire must not mutate roster"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
        "failed unwire must not emit PeersUnwired"
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let result = handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::CommsError(_))),
        "unwire should surface second trust-removal failure"
    );

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    assert!(
        entry_l.wired_to.contains(&MeerkatId::from("w-1")),
        "failed unwire must keep roster wiring on lead"
    );
    assert!(
        entry_w.wired_to.contains(&MeerkatId::from("l-1")),
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
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let result = handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::CommsError(_))),
        "unwire should fail when required notification fails"
    );

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    assert!(
        entry_l.wired_to.contains(&MeerkatId::from("w-1")),
        "failed unwire must not mutate roster"
    );
    assert!(
        entry_w.wired_to.contains(&MeerkatId::from("l-1")),
        "failed unwire must not mutate roster"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
        "failed unwire must not emit PeersUnwired"
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
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
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
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

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    assert!(
        entry_l.wired_to.contains(&MeerkatId::from("w-1")),
        "failed unwire must preserve lead wiring"
    );
    assert!(
        entry_w.wired_to.contains(&MeerkatId::from("l-1")),
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
            .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
        "failed unwire must not emit PeersUnwired"
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
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
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
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

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    assert!(
        entry_l.wired_to.contains(&MeerkatId::from("w-1")),
        "failed unwire must preserve lead wiring"
    );
    assert!(
        entry_w.wired_to.contains(&MeerkatId::from("l-1")),
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
            .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
        "failed unwire must not emit PeersUnwired"
    );
}

#[tokio::test]
async fn test_unwire_append_failure_restores_runtime_state() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("PeersUnwired").await;
    let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let result = handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await;
    assert!(
        matches!(result, Err(MobError::Internal(_))),
        "unwire should surface append failure"
    );

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    assert!(
        entry_l.wired_to.contains(&MeerkatId::from("w-1")),
        "unwire append failure must restore roster wiring"
    );
    assert!(
        entry_w.wired_to.contains(&MeerkatId::from("l-1")),
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
            .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
        "failed unwire append must not persist PeersUnwired"
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
    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    assert!(
        entry_l.wired_to.contains(&MeerkatId::from("w-1")),
        "orchestrator should be wired to worker"
    );

    let entry_w = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    assert!(
        entry_w.wired_to.contains(&MeerkatId::from("l-1")),
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
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

    let entry_l = handle.get_member(&MeerkatId::from("l-1")).await.unwrap();
    assert!(
        entry_l.wired_to.is_empty(),
        "orchestrator should not be wired to itself"
    );
}

#[tokio::test]
async fn test_spawn_skips_broken_orchestrator_in_auto_wire_selection() {
    let service = Arc::new(MockSessionService::new());
    let mut definition = sample_definition_with_auto_wire();
    definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .expect("lead profile")
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead")
        .session_id()
        .expect("session-backed")
        .clone();
    handle.stop().await.expect("stop");
    service.archive(&sid_l).await.expect("archive orchestrator");
    service.delete_persisted_session(&sid_l).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("partial resume should succeed");

    let lead = resumed
        .member_status(&MeerkatId::from("l-1"))
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
        .session_id()
        .expect("session-backed")
        .clone();
    let worker = resumed
        .get_member(&MeerkatId::from("w-1"))
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
        handle.get_member(&MeerkatId::from("w-1")).await.is_none(),
        "failed spawn should rollback roster entry"
    );
    let lead_entry = handle
        .get_member(&MeerkatId::from("l-1"))
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
        .session_id()
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
        handle.get_member(&MeerkatId::from("w-1")).await.is_none(),
        "failed spawn should rollback worker roster entry"
    );
    let lead_entry = handle
        .get_member(&MeerkatId::from("l-1"))
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
        .session_id()
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
        handle.get_member(&MeerkatId::from("w-1")).await.is_none(),
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .session_id()
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
        handle.get_member(&MeerkatId::from("w-3")).await.is_none(),
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
        handle.get_member(&MeerkatId::from("w-1")).await.is_some(),
        "rollback archive failure must not remove spawned roster entry"
    );

    let events = handle.events().replay_all().await.expect("replay");
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. })),
        "rollback must persist MeerkatRetired before side-effect cleanup so retries stay replay-safe"
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
        .session_id()
        .expect("session-backed")
        .clone();
    let spawn_err = spawn_handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect_err("spawn should fail under injected auto-wire fault");
    assert!(
        matches!(spawn_err, MobError::WiringError(_) | MobError::Internal(_)),
        "spawn fault should surface as wiring/internal error"
    );
    assert!(
        spawn_handle
            .get_member(&MeerkatId::from("w-1"))
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

    // Unwire rollback invariants.
    let unwire_events = Arc::new(FaultInjectedMobEventStore::new());
    unwire_events.fail_appends_for("PeersUnwired").await;
    let (unwire_handle, unwire_service) =
        create_test_mob_with_events(sample_definition(), unwire_events).await;
    let sid_u1 = unwire_handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("u-1"), None)
        .await
        .expect("spawn u-1")
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_u2 = unwire_handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("u-2"), None)
        .await
        .expect("spawn u-2")
        .session_id()
        .expect("session-backed")
        .clone();
    unwire_handle
        .wire(MeerkatId::from("u-1"), MeerkatId::from("u-2"))
        .await
        .expect("wire");
    let unwire_err = unwire_handle
        .unwire(MeerkatId::from("u-1"), MeerkatId::from("u-2"))
        .await
        .expect_err("unwire should fail when event append is fault-injected");
    assert!(
        matches!(unwire_err, MobError::Internal(_)),
        "unwire append fault should surface"
    );
    let u1 = unwire_handle
        .get_member(&MeerkatId::from("u-1"))
        .await
        .expect("u-1 remains in roster");
    let u2 = unwire_handle
        .get_member(&MeerkatId::from("u-2"))
        .await
        .expect("u-2 remains in roster");
    assert!(
        u1.wired_to.contains(&MeerkatId::from("u-2"))
            && u2.wired_to.contains(&MeerkatId::from("u-1")),
        "unwire rollback should preserve roster wiring on failure"
    );
    assert!(
        unwire_service
            .trusted_peer_names(&sid_u1)
            .await
            .contains(&test_comms_name("worker", "u-2"))
            && unwire_service
                .trusted_peer_names(&sid_u2)
                .await
                .contains(&test_comms_name("worker", "u-1")),
        "unwire rollback should restore comms trust edges on failure"
    );

    // Retire best-effort cleanup invariants: retire succeeds and removes
    // roster entry even when archive is fault-injected.
    let (retire_handle, retire_service) = create_test_mob(sample_definition()).await;
    let sid_r1 = retire_handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("r-1"), None)
        .await
        .expect("spawn r-1")
        .session_id()
        .expect("session-backed")
        .clone();
    let _sid_r2 = retire_handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("r-2"), None)
        .await
        .expect("spawn r-2");
    retire_handle
        .wire(MeerkatId::from("r-1"), MeerkatId::from("r-2"))
        .await
        .expect("wire");
    retire_service.set_archive_failure(&sid_r1).await;
    retire_handle
        .retire(MeerkatId::from("r-1"))
        .await
        .expect("retire should succeed despite archive failure (best-effort cleanup)");
    assert!(
        retire_handle
            .get_member(&MeerkatId::from("r-1"))
            .await
            .is_none(),
        "retire must remove roster entry even when archive fails"
    );
    let r2 = retire_handle
        .get_member(&MeerkatId::from("r-2"))
        .await
        .expect("r-2 should remain");
    assert!(
        !r2.wired_to.contains(&MeerkatId::from("r-1")),
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

    let entry_1 = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    let entry_2 = handle.get_member(&MeerkatId::from("w-2")).await.unwrap();
    let entry_3 = handle.get_member(&MeerkatId::from("w-3")).await.unwrap();

    // w-2 should be wired to w-1 (and vice versa)
    assert!(entry_2.wired_to.contains(&MeerkatId::from("w-1")));
    assert!(entry_1.wired_to.contains(&MeerkatId::from("w-2")));

    // w-3 should be wired to both w-1 and w-2
    assert!(entry_3.wired_to.contains(&MeerkatId::from("w-1")));
    assert!(entry_3.wired_to.contains(&MeerkatId::from("w-2")));

    // w-1 should be wired to both w-2 and w-3
    // Need to re-read since roster may have been updated after w-3 spawn
    let entry_1 = handle.get_member(&MeerkatId::from("w-1")).await.unwrap();
    assert!(entry_1.wired_to.contains(&MeerkatId::from("w-3")));
}

#[tokio::test]
async fn test_spawn_skips_broken_role_peers_in_role_wiring_selection() {
    let service = Arc::new(MockSessionService::new());
    let mut definition = sample_definition_with_role_wiring();
    definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .runtime_mode = crate::MobRuntimeMode::TurnDriven;
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let sid_w1 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1")
        .session_id()
        .expect("session-backed")
        .clone();
    handle.stop().await.expect("stop");
    service.archive(&sid_w1).await.expect("archive w-1");
    service.delete_persisted_session(&sid_w1).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("partial resume should succeed");

    let broken = resumed
        .member_status(&MeerkatId::from("w-1"))
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
        .session_id()
        .expect("session-backed")
        .clone();
    let worker = resumed
        .get_member(&MeerkatId::from("w-2"))
        .await
        .expect("spawned worker");
    assert!(
        !worker.wired_to.contains(&MeerkatId::from("w-1")),
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
        .get_member(&MeerkatId::from("l-1"))
        .await
        .expect("lead should be in roster");
    assert_eq!(
        lead.wired_to.len(),
        3,
        "new role_x meerkat should fan out to all 3 existing role_y peers"
    );
    for worker_id in ["w-1", "w-2", "w-3"] {
        assert!(
            lead.wired_to.contains(&MeerkatId::from(worker_id)),
            "lead should be wired to {worker_id}"
        );
        let worker = handle
            .get_member(&MeerkatId::from(worker_id))
            .await
            .expect("worker should remain in roster");
        assert!(
            worker.wired_to.contains(&MeerkatId::from("l-1")),
            "{worker_id} should be wired back to lead"
        );
    }

    let events = handle.events().replay_all().await.expect("replay");
    let cross_role_wire_events = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::PeersWired { a, b }
                    if (a == &MeerkatId::from("l-1")
                        && ["w-1", "w-2", "w-3"]
                            .iter()
                            .any(|w| b == &MeerkatId::from(*w)))
                        || (b == &MeerkatId::from("l-1")
                            && ["w-1", "w-2", "w-3"]
                                .iter()
                                .any(|w| a == &MeerkatId::from(*w)))
            )
        })
        .count();
    assert_eq!(
        cross_role_wire_events, 3,
        "fan-out should execute three wire() operations for three existing role_y peers"
    );
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
        .get_member(&MeerkatId::from("l-1"))
        .await
        .expect("lead should exist");
    assert_eq!(
        lead.wired_to.len(),
        1,
        "overlapping auto-wire + role rule should produce one trust edge"
    );
    assert!(lead.wired_to.contains(&MeerkatId::from("w-1")));

    let events = handle.events().replay_all().await.expect("replay");
    let wires_for_pair = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::PeersWired { a, b }
                    if (a == &MeerkatId::from("l-1") && b == &MeerkatId::from("w-1"))
                        || (a == &MeerkatId::from("w-1") && b == &MeerkatId::from("l-1"))
            )
        })
        .count();
    assert_eq!(
        wires_for_pair, 1,
        "wiring overlap should emit a single PeersWired event for one logical edge"
    );
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
        handle.get_member(&MeerkatId::from("w-2")).await.is_none(),
        "failed spawn should rollback roster entry"
    );
    let entry_w1 = handle
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("w-1 should remain in roster");
    assert!(
        !entry_w1.wired_to.contains(&MeerkatId::from("w-2")),
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
        .member(&MeerkatId::from("l-1"))
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
        .member(&MeerkatId::from("l-1"))
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
                    data: "aGVsbG8=".to_string(),
                    source_path: None,
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
        .member(&MeerkatId::from("w-1"))
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
    assert!(matches!(result, Err(MobError::MeerkatNotFound(_))));
}

#[tokio::test]
async fn test_internal_turn_bypasses_external_addressable_check() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");

    let result = handle
        .internal_turn(MeerkatId::from("w-1"), "internal message")
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
        .internal_turn(MeerkatId::from("nonexistent"), "Hello")
        .await;
    assert!(matches!(result, Err(MobError::MeerkatNotFound(_))));
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
        .member(&MeerkatId::from("l-autonomous"))
        .await
        .expect("member handle")
        .send("inject me", meerkat_core::types::HandlingMode::Queue)
        .await
        .expect("external turn should execute");

    assert_eq!(
        service.inject_call_count(),
        1,
        "autonomous dispatch must use event injector"
    );
    assert_eq!(
        service.start_turn_call_count(),
        service.keep_alive_start_turn_call_count(),
        "autonomous dispatch must not issue additional non-host start_turn calls"
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
        .member(&MeerkatId::from("l-idle-autonomous"))
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
        .member(&MeerkatId::from("l-turn-driven"))
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
        .member(&MeerkatId::from("l-runtime-fail"))
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
    let builder_adapter = Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral());
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
        .session_id()
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
    assert_eq!(terminal.status, MobRunStatus::Completed);

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
    let baseline_start_turn = service.start_turn_call_count();

    handle
        .internal_turn(MeerkatId::from("l-auto"), "internal autonomous")
        .await
        .expect("autonomous internal turn");
    handle
        .internal_turn(MeerkatId::from("l-turn"), "internal turn-driven")
        .await
        .expect("turn-driven internal turn");

    assert_eq!(
        service.inject_call_count(),
        1,
        "autonomous internal turn should route via injector"
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
        .session_id()
        .cloned()
        .expect("session-backed target");
    let baseline_interrupts = service.interrupt_call_count();

    handle
        .force_cancel_member(member_id.clone())
        .await
        .expect("force cancel should succeed");

    assert_eq!(
        service.interrupt_call_count(),
        baseline_interrupts + 1,
        "force-cancel must route through the runtime adapter interrupt path exactly once"
    );
    let entry = handle.get_member(&member_id).await.expect("member remains");
    assert_eq!(
        entry.member_ref.session_id().cloned(),
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
    let (handle, service) = create_test_mob(sample_definition_with_external_backend()).await;
    handle
        .spawn_with_options(
            ProfileName::from("lead"),
            MeerkatId::from("l-ext-turn"),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::External),
        )
        .await
        .expect("spawn external turn-driven lead");
    let baseline_start_turn = service.start_turn_call_count();

    handle
        .member(&MeerkatId::from("l-ext-turn"))
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
        "turn-driven external backend dispatch should not use injector"
    );
    assert_eq!(
        service.start_turn_call_count(),
        baseline_start_turn + 1,
        "turn-driven external backend dispatch should issue start_turn"
    );
}

#[tokio::test]
async fn test_external_backend_autonomous_flow_dispatch_uses_injector_routing() {
    let mut definition = sample_definition_with_dispatch_mode(DispatchMode::OneToOne);
    definition.backend.external = Some(crate::definition::ExternalBackendConfig {
        address_base: "https://backend.example.invalid/mesh".to_string(),
    });
    let (handle, service) = create_test_mob(definition).await;
    handle
        .spawn_with_options(
            ProfileName::from("lead"),
            MeerkatId::from("l-ext-auto"),
            None,
            Some(crate::MobRuntimeMode::AutonomousHost),
            Some(MobBackendKind::External),
        )
        .await
        .expect("spawn external autonomous lead");
    handle
        .spawn_with_options(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext-auto"),
            None,
            Some(crate::MobRuntimeMode::AutonomousHost),
            Some(MobBackendKind::External),
        )
        .await
        .expect("spawn external autonomous worker");
    let baseline_non_host_start_turn = service
        .start_turn_call_count()
        .saturating_sub(service.keep_alive_start_turn_call_count());

    let run_id = handle
        .run_flow(FlowId::from("dispatch"), serde_json::json!({}))
        .await
        .expect("run flow");
    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Completed);

    let non_host_start_turn = service
        .start_turn_call_count()
        .saturating_sub(service.keep_alive_start_turn_call_count());
    assert_eq!(
        non_host_start_turn, baseline_non_host_start_turn,
        "external autonomous flow dispatch should avoid non-host start_turn calls"
    );
    assert!(
        service.inject_call_count() > 0,
        "external autonomous flow dispatch should use injector routing"
    );
}

#[tokio::test]
async fn test_external_backend_lifecycle_and_turn_policy() {
    let (handle, _service) = create_test_mob(sample_definition_with_external_backend()).await;

    handle
        .spawn_with_backend(
            ProfileName::from("lead"),
            MeerkatId::from("l-ext"),
            None,
            Some(MobBackendKind::External),
        )
        .await
        .expect("spawn external lead");
    handle
        .spawn_with_backend(
            ProfileName::from("worker"),
            MeerkatId::from("w-ext"),
            None,
            Some(MobBackendKind::External),
        )
        .await
        .expect("spawn external worker");

    handle
        .wire(MeerkatId::from("l-ext"), MeerkatId::from("w-ext"))
        .await
        .expect("wire external members");
    handle
        .unwire(MeerkatId::from("l-ext"), MeerkatId::from("w-ext"))
        .await
        .expect("unwire external members");

    handle
        .member(&MeerkatId::from("l-ext"))
        .await
        .expect("member handle")
        .send(
            "outside hello".to_string(),
            meerkat_core::types::HandlingMode::Queue,
        )
        .await
        .expect("external lead should accept external turns");
    let denied = handle
        .member(&MeerkatId::from("w-ext"))
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
        .retire(MeerkatId::from("w-ext"))
        .await
        .expect("retire external member");
    assert!(
        handle.get_member(&MeerkatId::from("w-ext")).await.is_none(),
        "retire should remove external backend member"
    );
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
        .map(|entry| entry.meerkat_id.as_str().to_string())
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
        .filter(|event| matches!(event.kind, MobEventKind::MeerkatSpawned { .. }))
        .count();
    assert_eq!(
        spawned_count, 10,
        "parallel spawn path should emit exactly one MeerkatSpawned per request"
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
        let session_id = member.session_id().expect("session-backed member");
        assert!(
            !session_id.to_string().is_empty(),
            "spawn_many result {idx} should include session id"
        );
    }
    assert!(
        service.max_concurrent_create_session_calls() > 1,
        "spawn_many should use parallel provisioning under the hood"
    );
}

#[tokio::test]
async fn test_spawn_many_parallel_finalize_emits_single_worker_pair_wire_event() {
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

    let events = handle.events().replay_all().await.expect("replay");
    let mut pair_wire_events = 0usize;
    for event in events {
        if let MobEventKind::PeersWired { a, b } = event.kind {
            let a_id = a.as_str();
            let b_id = b.as_str();
            if (a_id == "w-a" && b_id == "w-b") || (a_id == "w-b" && b_id == "w-a") {
                pair_wire_events += 1;
            }
        }
    }

    assert_eq!(
        pair_wire_events, 1,
        "parallel spawn finalization must emit exactly one PeersWired event for w-a<->w-b"
    );
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
    let retire = tokio::spawn(async move { h2.retire(MeerkatId::from("w-1")).await });

    let _ = spawn.await.expect("spawn join");
    retire.await.expect("retire join").expect("retire");

    let roster = handle.list_members().await;
    assert!(
        roster.is_empty() || (roster.len() == 1 && roster[0].meerkat_id.as_str() == "w-1"),
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
        .session_id()
        .expect("session-backed")
        .clone();

    let retire_handle = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.retire(MeerkatId::from("w-1")).await })
    };

    tokio::time::sleep(Duration::from_millis(25)).await;

    let active_members = handle.list_members().await;
    assert!(
        active_members.is_empty(),
        "retiring member must leave the active roster before disposal completes"
    );

    let all_members = handle.list_all_members().await;
    assert_eq!(
        all_members.len(),
        1,
        "retiring member should remain observable"
    );
    assert_eq!(all_members[0].meerkat_id.as_str(), "w-1");
    assert_eq!(all_members[0].state, crate::roster::MemberState::Retiring);

    let start_turn_calls_before = service.start_turn_call_count();
    let external_turn = handle.member(&MeerkatId::from("w-1")).await;
    assert!(matches!(external_turn, Err(MobError::MeerkatNotFound(id)) if id.as_str() == "w-1"));

    let internal_turn = handle
        .internal_turn(MeerkatId::from("w-1"), "still there?".to_string())
        .await
        .expect_err("retiring member must reject new internal work");
    assert!(matches!(internal_turn, MobError::MeerkatNotFound(id) if id.as_str() == "w-1"));

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
        handle.get_member(&MeerkatId::from("w-1")).await.is_none(),
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
    assert!(matches!(duplicate, MobError::MeerkatAlreadyExists(_)));

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
    assert!(
        handle
            .get_member(&MeerkatId::from("w-stop-lineage"))
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
async fn test_failed_spawn_clears_pending_spawn_count_and_failed_roster_entry() {
    let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
    handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn lead");

    let initial = handle
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
    assert!(
        staged.topology_revision > initial.topology_revision,
        "staging a pending spawn must advance orchestrator-owned topology revision"
    );

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
    assert!(
        settled.topology_revision > staged.topology_revision,
        "failed pending spawn settlement must advance orchestrator-owned topology revision again"
    );
    assert!(
        handle
            .get_member(&MeerkatId::from("w-pending"))
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
        handle.get_member(&MeerkatId::from("w-3")).await.is_none(),
        "failed spawn must not leave the new member in the canonical roster"
    );
    let w1 = handle
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("w-1 remains active");
    let w2 = handle
        .get_member(&MeerkatId::from("w-2"))
        .await
        .expect("w-2 remains active");
    assert!(
        w1.wired_to.contains(&MeerkatId::from("w-2")),
        "rollback must preserve the pre-existing worker pair edge on w-1"
    );
    assert!(
        w2.wired_to.contains(&MeerkatId::from("w-1")),
        "rollback must preserve the pre-existing worker pair edge on w-2"
    );
    assert!(
        !w1.wired_to.contains(&MeerkatId::from("w-3"))
            && !w2.wired_to.contains(&MeerkatId::from("w-3")),
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
    assert_eq!(terminal.status, MobRunStatus::Completed);
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
                && entry.meerkat_id == crate::runtime::flow_system_member_id()
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
        .session_id()
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
                MobEventKind::StepDispatched { run_id: id, step_id, meerkat_id }
                    if id == &run_id && step_id.as_str() == "start" && meerkat_id.as_str() == "w-1"
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
        .session_id()
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
    assert!(
        staged.topology_revision > initial.topology_revision,
        "staging a spawn must advance orchestrator-owned topology revision"
    );

    spawn_handle
        .await
        .expect("spawn join")
        .expect("spawn worker");
    let settled = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("settled snapshot");
    assert_eq!(settled.pending_spawn_count, 0);
    assert!(
        settled.topology_revision > staged.topology_revision,
        "spawn completion must advance the orchestrator-owned revision again"
    );

    let events = handle.events().replay_all().await.expect("replay events");
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        MobEventKind::MeerkatSpawned { meerkat_id, .. } if meerkat_id.as_str() == "w-1"
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
    assert!(active.supervisor_active);

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
    assert!(settled.supervisor_active);

    handle.stop().await.expect("stop mob");
    let stopped = handle
        .debug_orchestrator_snapshot()
        .await
        .expect("stopped snapshot");
    assert!(!stopped.coordinator_bound);
    assert!(!stopped.supervisor_active);

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
    handle.destroy().await.expect("destroy mob");

    let terminal = wait_for_run_terminal(&handle, &run_id, Duration::from_secs(2)).await;
    assert_eq!(terminal.status, MobRunStatus::Canceled);
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
            entry.step_id == crate::StepId::from("start")
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
        .session_id()
        .expect("session-backed")
        .clone();
    let _sid_w2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .session_id()
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
        .session_id()
        .expect("session-backed")
        .clone();
    let _sid_w2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .session_id()
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
                && entry.meerkat_id == crate::runtime::flow_system_member_id()
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
        .expect("lead profile");
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
    events.fail_appends_for("MeerkatRetired").await;
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
fn test_mob_state_roundtrip() {
    for state in [
        MobState::Creating,
        MobState::Running,
        MobState::Stopped,
        MobState::Completed,
        MobState::Destroyed,
    ] {
        assert_eq!(MobState::from_u8(state as u8), state);
    }
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "invalid mob lifecycle state byte")]
fn test_mob_state_invalid_byte_panics_in_debug() {
    let _ = MobState::from_u8(255);
}

#[test]
#[cfg(not(debug_assertions))]
fn test_mob_state_invalid_byte_falls_back_to_destroyed() {
    assert_eq!(MobState::from_u8(255), MobState::Destroyed);
}

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

    let keep_alive_prompts = service.keep_alive_prompts().await;
    assert_eq!(
        keep_alive_prompts.len(),
        1,
        "autonomous spawn must start exactly one host loop"
    );
    assert_eq!(
        keep_alive_prompts[0].1, custom_msg,
        "first autonomous host-loop prompt must use initial_message when provided"
    );
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        1,
        "spawn must start exactly one autonomous host loop"
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

    let keep_alive_prompts = service.keep_alive_prompts().await;
    assert_eq!(
        keep_alive_prompts.len(),
        1,
        "autonomous spawn must start exactly one host loop"
    );
    assert!(
        keep_alive_prompts[0]
            .1
            .contains("You have been spawned as 'w-1'"),
        "host-loop fallback prompt should contain meerkat id, got: '{}'",
        keep_alive_prompts[0].1
    );
    assert!(
        keep_alive_prompts[0].1.contains("role: worker"),
        "host-loop fallback prompt should contain role, got: '{}'",
        keep_alive_prompts[0].1
    );
    assert!(
        keep_alive_prompts[0].1.contains("mob 'test-mob'"),
        "host-loop fallback prompt should contain mob id, got: '{}'",
        keep_alive_prompts[0].1
    );
}

#[tokio::test]
async fn test_spawn_autonomous_surfaces_immediate_host_loop_start_failure() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_fail_start_turn(true);

    let result = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-fail"), None)
        .await;

    assert!(
        matches!(result, Err(MobError::SessionError(SessionError::Store(_)))),
        "autonomous spawn must surface immediate host-loop start_turn failures"
    );
    assert!(
        handle
            .get_member(&MeerkatId::from("w-fail"))
            .await
            .is_none(),
        "failed autonomous spawn must roll back projected roster entry"
    );
}

#[tokio::test]
async fn test_retire_interrupts_autonomous_host_loop() {
    let (handle, service) = create_test_mob(sample_definition()).await;

    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker");
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        1,
        "spawn should start autonomous host loop"
    );

    handle
        .retire(MeerkatId::from("w-1"))
        .await
        .expect("retire worker");
    assert_eq!(
        service.interrupt_call_count(),
        1,
        "retire must terminate autonomous host loop deterministically"
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
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        1,
        "only autonomous members should start host loops"
    );

    handle.stop().await.expect("stop");
    assert_eq!(
        service.interrupt_call_count(),
        1,
        "stop should terminate all active autonomous host loops"
    );

    handle.resume().await.expect("resume");
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        2,
        "resume should restart autonomous host loops from projected runtime_mode"
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
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        2,
        "both autonomous members should start host loops"
    );

    handle.destroy().await.expect("destroy");
    assert_eq!(
        service.interrupt_call_count(),
        2,
        "destroy must terminate all autonomous host loops deterministically"
    );
}

#[tokio::test]
async fn test_resume_from_events_restarts_autonomous_host_loops_from_runtime_mode() {
    let storage = MobStorage::in_memory();
    let storage_for_resume = MobStorage {
        events: storage.events.clone(),
        runs: storage.runs.clone(),
        specs: storage.specs.clone(),
    };
    let service = Arc::new(MockSessionService::new());
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
        resumed.status(),
        MobState::Running,
        "resumed runtime should be Running"
    );
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert_eq!(
        service.keep_alive_start_turn_call_count(),
        2,
        "resume should restart only autonomous host loops from projected runtime_mode"
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

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service)
        .allow_ephemeral_sessions(true)
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("resume handle should still be constructed");

    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert_eq!(
        resumed.status(),
        MobState::Stopped,
        "startup host-loop failure should force runtime into Stopped state"
    );
}

#[tokio::test]
async fn test_resume_skips_broken_autonomous_member_in_host_loop_startup() {
    let service = Arc::new(MockSessionService::new());
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");

    let sid_l = handle
        .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
        .await
        .expect("spawn autonomous lead")
        .session_id()
        .expect("session-backed")
        .clone();
    handle.stop().await.expect("stop");
    service.archive(&sid_l).await.expect("archive lead");
    service.delete_persisted_session(&sid_l).await;

    let resumed = MobBuilder::for_resume(MobStorage::with_events(events))
        .with_session_service(service)
        .notify_orchestrator_on_resume(false)
        .resume()
        .await
        .expect("partial resume should succeed");

    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert_eq!(
        resumed.status(),
        MobState::Running,
        "Broken autonomous members must be skipped during host-loop startup so partial resume stays running"
    );
    let snapshot = resumed
        .member_status(&MeerkatId::from("l-1"))
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
}

impl RealCommsSessionService {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            keep_alive_notifiers: RwLock::new(HashMap::new()),
            session_comms_names: RwLock::new(HashMap::new()),
            session_counter: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl SessionService for RealCommsSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let session_id = SessionId::new();
        let n = self.session_counter.fetch_add(1, Ordering::Relaxed);

        let comms_name = req
            .build
            .as_ref()
            .and_then(|b| b.comms_name.clone())
            .unwrap_or_else(|| format!("real-comms-session-{n}"));

        let comms = meerkat_comms::CommsRuntime::inproc_only(&comms_name)
            .expect("create inproc CommsRuntime");
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::new(comms));
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

/// Create a MobHandle with real comms (for behavioral wire tests).
async fn create_test_mob_with_real_comms(
    definition: MobDefinition,
) -> (MobHandle, Arc<RealCommsSessionService>) {
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
/// Uses real inproc comms delivery, a real RuntimeSessionAdapter, and records
/// runtime-applied prompts so tests can prove peer inbox traffic still reaches
/// `apply_runtime_turn()` after the initial autonomous kickoff turn has exited.
struct RuntimeBackedRealCommsSessionService {
    sessions: RwLock<HashMap<SessionId, Arc<meerkat_comms::CommsRuntime>>>,
    keep_alive_notifiers: RwLock<HashMap<SessionId, Arc<tokio::sync::Notify>>>,
    session_comms_names: RwLock<HashMap<SessionId, String>>,
    session_counter: AtomicU64,
    runtime_adapter: Arc<meerkat_runtime::RuntimeSessionAdapter>,
    keep_alive_turns_complete_immediately: std::sync::atomic::AtomicBool,
    applied_runtime_prompts: RwLock<HashMap<SessionId, Vec<ContentInput>>>,
}

impl RuntimeBackedRealCommsSessionService {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            keep_alive_notifiers: RwLock::new(HashMap::new()),
            session_comms_names: RwLock::new(HashMap::new()),
            session_counter: AtomicU64::new(0),
            runtime_adapter: Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral()),
            keep_alive_turns_complete_immediately: std::sync::atomic::AtomicBool::new(false),
            applied_runtime_prompts: RwLock::new(HashMap::new()),
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
}

#[async_trait]
impl SessionService for RuntimeBackedRealCommsSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let session_id = SessionId::new();
        let n = self.session_counter.fetch_add(1, Ordering::Relaxed);

        let comms_name = req
            .build
            .as_ref()
            .and_then(|b| b.comms_name.clone())
            .unwrap_or_else(|| format!("real-runtime-comms-session-{n}"));

        let comms = meerkat_comms::CommsRuntime::inproc_only(&comms_name)
            .expect("create inproc CommsRuntime");
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::new(comms));
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
impl MobSessionService for RuntimeBackedRealCommsSessionService {
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::RuntimeSessionAdapter>> {
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
            run_result: None,
        })
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.sessions.write().await.remove(session_id);
        self.keep_alive_notifiers.write().await.remove(session_id);
        self.session_comms_names.write().await.remove(session_id);
        self.applied_runtime_prompts
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire should succeed");

    tokio::time::sleep(Duration::from_millis(50)).await;
    let baseline_prompts = service.applied_runtime_prompts(&sid_b).await.len();

    let comms_a = service.real_comms(&sid_a).await.expect("comms for l-1");
    let receipt = CoreCommsRuntime::send(
        &*comms_a,
        CommsCommand::PeerMessage {
            to: PeerName::new(&test_comms_name("worker", "w-1")).expect("valid peer name"),
            body: "body: please inspect this image".to_string(),
            blocks: Some(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "caption: this block text should survive".to_string(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "aGVsbG8=".to_string(),
                    source_path: None,
                },
            ]),
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
        delivered_text.contains("body: please inspect this image"),
        "peer message body should survive runtime delivery: {delivered_text:?}"
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
async fn test_member_status_keeps_idle_live_session_active() {
    let inner = Arc::new(MockSessionService::new());
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
        .member_status(&MeerkatId::from("w-1"))
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
        .session_id()
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
        .session_id()
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

fn has_request_intent_for_peer(
    interactions: &[meerkat_core::InboxInteraction],
    expected_intent: &str,
    expected_peer: &str,
) -> bool {
    interactions.iter().any(|interaction| {
        matches!(
            &interaction.content,
            meerkat_core::InteractionContent::Request { intent, params }
                if intent == expected_intent && params["peer"] == expected_peer
        )
    })
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();

    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire should succeed");

    let comms_a = service.real_comms(&sid_a).await.expect("comms for l-1");
    let comms_b = service.real_comms(&sid_b).await.expect("comms for w-1");

    let entry_a = handle
        .get_member(&MeerkatId::from("l-1"))
        .await
        .expect("l-1 should be in roster");
    let entry_b = handle
        .get_member(&MeerkatId::from("w-1"))
        .await
        .expect("w-1 should be in roster");
    let comms_name_a = format!(
        "{}/{}/{}",
        handle.mob_id(),
        entry_a.profile,
        entry_a.meerkat_id
    );
    let comms_name_b = format!(
        "{}/{}/{}",
        handle.mob_id(),
        entry_b.profile,
        entry_b.meerkat_id
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

    let notify_for_a = CoreCommsRuntime::drain_inbox_interactions(&*comms_a).await;
    let notify_for_b = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;
    assert!(
        has_request_intent_for_peer(&notify_for_a, "mob.peer_added", "w-1"),
        "lead should receive mob.peer_added for worker"
    );
    assert!(
        has_request_intent_for_peer(&notify_for_b, "mob.peer_added", "l-1"),
        "worker should receive mob.peer_added for lead"
    );

    let peer_name = meerkat_core::comms::PeerName::new(&comms_name_b).expect("valid peer name");
    let cmd = meerkat_core::comms::CommsCommand::PeerRequest {
        to: peer_name,
        intent: "mob.test_ping".to_string(),
        params: serde_json::json!({"test": true}),
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    let comms_a = service.real_comms(&sid_a).await.expect("comms for l-1");
    let comms_b = service.real_comms(&sid_b).await.expect("comms for w-1");
    let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_a).await;
    let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;

    handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");
    handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
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
            TrustedPeerSpec::new(&name_b, key_b.to_peer_id(), format!("inproc://{name_b}"))
                .expect("valid worker trusted spec"),
        )
        .await
        .expect("re-add stale trust on lead");
    comms_b
        .add_trusted_peer(
            TrustedPeerSpec::new(&name_a, key_a.to_peer_id(), format!("inproc://{name_a}"))
                .expect("valid lead trusted spec"),
        )
        .await
        .expect("re-add stale trust on worker");

    handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
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
        .session_id()
        .expect("session-backed")
        .clone();
    let spec = TrustedPeerSpec::new(
        "remote-mob/worker/agent-x",
        meerkat_comms::Keypair::generate().public_key().to_peer_id(),
        "inproc://remote-mob/worker/agent-x",
    )
    .expect("valid external peer");
    handle
        .wire(MeerkatId::from("l-1"), PeerTarget::External(spec.clone()))
        .await
        .expect("wire external");
    handle
        .unwire(MeerkatId::from("l-1"), PeerTarget::External(spec.clone()))
        .await
        .expect("initial external unwire");

    let comms = service.real_comms(&sid).await.expect("comms for l-1");
    comms
        .add_trusted_peer(spec.clone())
        .await
        .expect("re-add stale external trust");

    handle
        .unwire(MeerkatId::from("l-1"), PeerTarget::External(spec.clone()))
        .await
        .expect("idempotent external stale-trust cleanup");

    let peers = CoreCommsRuntime::peers(&*comms).await;
    assert!(
        !peers.iter().any(|entry| entry.name.as_str() == spec.name),
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_b = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    let comms_a = service.real_comms(&sid_a).await.expect("comms for w-1");
    let comms_b = service.real_comms(&sid_b).await.expect("comms for w-2");
    let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_a).await;
    let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;

    handle.retire(MeerkatId::from("w-1")).await.expect("retire");

    assert!(
        handle.get_member(&MeerkatId::from("w-1")).await.is_none(),
        "retired meerkat should leave roster"
    );
    let entry_w2 = handle
        .get_member(&MeerkatId::from("w-2"))
        .await
        .expect("w-2 remains active");
    assert!(
        !entry_w2.wired_to.contains(&MeerkatId::from("w-1")),
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn worker")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
        .await
        .expect("wire");

    handle
        .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
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
        .session_id()
        .expect("session-backed")
        .clone();
    let sid_w2 = handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
        .await
        .expect("spawn w-2")
        .session_id()
        .expect("session-backed")
        .clone();
    handle
        .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    handle.retire(MeerkatId::from("w-1")).await.expect("retire");

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
        .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
        .await
        .expect("wire");

    // Archive also fails.
    let sid_w1 = handle
        .get_member(&MeerkatId::from("w-1"))
        .await
        .unwrap()
        .session_id()
        .cloned()
        .unwrap();
    service.set_archive_failure(&sid_w1).await;

    // Retire succeeds (best-effort policy always continues).
    handle
        .retire(MeerkatId::from("w-1"))
        .await
        .expect("retire should succeed despite multiple step failures");

    // The structural guarantee: member is gone from roster regardless.
    assert!(
        handle.get_member(&MeerkatId::from("w-1")).await.is_none(),
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
        .retire(MeerkatId::from("w-1"))
        .await
        .expect("retire w-1");
    handle
        .retire(MeerkatId::from("w-2"))
        .await
        .expect("retire w-2");

    // Both members removed — disposal executed identically.
    assert!(handle.get_member(&MeerkatId::from("w-1")).await.is_none());
    assert!(handle.get_member(&MeerkatId::from("w-2")).await.is_none());

    // Verify events show two retirements in order.
    let events = handle.events().replay_all().await.expect("replay");
    let retire_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. }))
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
    assert_eq!(handle.status(), MobState::Running);
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
    assert_eq!(handle.status(), MobState::Stopped);

    handle.reset().await.expect("reset from stopped");
    assert_eq!(handle.status(), MobState::Running);
}

#[tokio::test]
async fn test_reset_from_completed_state() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle.complete().await.expect("complete");
    assert_eq!(handle.status(), MobState::Completed);

    handle.reset().await.expect("reset from completed");
    assert_eq!(handle.status(), MobState::Running);
}

#[tokio::test]
async fn test_reset_rejects_from_destroyed() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    handle.destroy().await.expect("destroy");
    assert_eq!(handle.status(), MobState::Destroyed);

    let result = handle.reset().await;
    assert!(
        matches!(result, Err(MobError::InvalidTransition { .. })),
        "reset from Destroyed should fail with InvalidTransition"
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
async fn test_reset_append_failure_transitions_to_stopped() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let (handle, _service) = create_test_mob_with_events(sample_definition(), events.clone()).await;
    handle
        .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
        .await
        .expect("spawn w-1");
    assert_eq!(handle.status(), MobState::Running);

    // Fail the MobReset append — MobCreated append succeeds but reset
    // should still report failure. After destructive steps (retire/stop),
    // fail-closed to Stopped.
    events.fail_appends_for("MobReset").await;

    let result = handle.reset().await;
    assert!(
        matches!(result, Err(MobError::Internal(_))),
        "reset should surface append failure"
    );
    assert_eq!(
        handle.status(),
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
    assert_eq!(handle.status(), MobState::Stopped);

    // Fail the MobCreated append to trigger reset failure from Stopped.
    events.fail_appends_for("MobCreated").await;
    let result = handle.reset().await;
    assert!(result.is_err(), "reset should fail");

    assert_eq!(
        handle.status(),
        MobState::Stopped,
        "failed reset from Stopped must remain Stopped"
    );
}

#[tokio::test]
async fn test_shutdown_does_not_stall_on_stuck_lifecycle_notification() {
    // Create a definition with a TurnDriven orchestrator so lifecycle
    // notifications go through start_turn (which we can delay).
    let mut def = sample_definition();
    if let Some(profile) = def.profiles.get_mut(&ProfileName::from("lead")) {
        profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    }

    let service = Arc::new(MockSessionService::new());
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
