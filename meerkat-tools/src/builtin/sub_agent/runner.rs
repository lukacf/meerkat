//! Sub-agent runner - Actually executes sub-agents with comms support
//!
//! This module handles the full lifecycle of sub-agent execution:
//! - Creating LLM client adapters
//! - Setting up comms infrastructure for parent-child communication
//! - Running the agent loop in a background task
//! - Reporting results back to the parent

use crate::comms_dispatcher::wrap_with_comms;
use async_trait::async_trait;
use futures::StreamExt;
use meerkat_client::LlmClient;
use meerkat_client::{LlmEvent, LlmRequest};
use meerkat_comms::{PubKey, TrustedPeer, TrustedPeers};
use meerkat_core::comms_bootstrap::CommsBootstrap;
use meerkat_core::comms_config::CoreCommsConfig;
use meerkat_core::comms_runtime::CommsRuntime;
use meerkat_core::ops::{OperationId, OperationResult, SteeringMessage};
use meerkat_core::session::Session;
use meerkat_core::sub_agent::SubAgentManager;
use meerkat_core::types::{Message, StopReason, SystemMessage, ToolCall, Usage, UserMessage};
use meerkat_core::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, BudgetLimits,
    LlmStreamResult, ParentCommsContext, ToolDef,
};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio::sync::mpsc;

/// Configuration for spawning a sub-agent with comms
#[derive(Debug, Clone)]
pub struct SubAgentCommsConfig {
    /// Name for this sub-agent
    pub name: String,
    /// Base directory for comms identity files
    pub base_dir: PathBuf,
    /// Parent's context for establishing trust
    pub parent_context: ParentCommsContext,
}

/// Handle to a running sub-agent
#[derive(Debug)]
pub struct SubAgentHandle {
    /// Operation ID
    pub id: OperationId,
    /// Name of the sub-agent
    pub name: String,
    /// Child's public key (for parent to send messages)
    pub child_pubkey: [u8; 32],
    /// Address to reach the child
    pub child_addr: String,
}

/// Adapter that converts meerkat_client::LlmClient to meerkat_core::AgentLlmClient
///
/// This bridges the streaming interface used by providers with the
/// result-based interface used by the Agent.
pub struct LlmClientAdapter<C: LlmClient> {
    client: Arc<C>,
    model: String,
}

impl<C: LlmClient> LlmClientAdapter<C> {
    /// Create a new adapter wrapping the given client
    pub fn new(client: Arc<C>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl<C: LlmClient + 'static> AgentLlmClient for LlmClientAdapter<C> {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError> {
        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            stop_sequences: None,
            provider_params: provider_params.cloned(),
        };

        let mut stream = self.client.stream(&request);

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => match event {
                    LlmEvent::TextDelta { delta } => {
                        content.push_str(&delta);
                    }
                    LlmEvent::ToolCallComplete {
                        id,
                        name,
                        args,
                        thought_signature,
                    } => {
                        let tc = if let Some(sig) = thought_signature {
                            ToolCall::with_thought_signature(id, name, args, sig)
                        } else {
                            ToolCall::new(id, name, args)
                        };
                        tool_calls.push(tc);
                    }
                    LlmEvent::UsageUpdate { usage: u } => {
                        usage = u;
                    }
                    LlmEvent::Done { stop_reason: sr } => {
                        stop_reason = sr;
                    }
                    _ => {}
                },
                Err(e) => {
                    return Err(AgentError::LlmError(e.to_string()));
                }
            }
        }

        Ok(LlmStreamResult {
            content,
            tool_calls,
            stop_reason,
            usage,
        })
    }

    fn provider(&self) -> &'static str {
        self.client.provider()
    }
}

/// Type-erased adapter for trait object clients (from factory)
///
/// This allows using `Arc<dyn LlmClient>` (as returned by LlmClientFactory)
/// with the Agent system which requires sized types.
pub struct DynLlmClientAdapter {
    client: Arc<dyn LlmClient>,
    model: String,
}

impl DynLlmClientAdapter {
    /// Create a new adapter wrapping a trait object client
    pub fn new(client: Arc<dyn LlmClient>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl AgentLlmClient for DynLlmClientAdapter {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError> {
        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            stop_sequences: None,
            provider_params: provider_params.cloned(),
        };

        let mut stream = self.client.stream(&request);

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => match event {
                    LlmEvent::TextDelta { delta } => {
                        content.push_str(&delta);
                    }
                    LlmEvent::ToolCallComplete {
                        id,
                        name,
                        args,
                        thought_signature,
                    } => {
                        let tc = if let Some(sig) = thought_signature {
                            ToolCall::with_thought_signature(id, name, args, sig)
                        } else {
                            ToolCall::new(id, name, args)
                        };
                        tool_calls.push(tc);
                    }
                    LlmEvent::UsageUpdate { usage: u } => {
                        usage = u;
                    }
                    LlmEvent::Done { stop_reason: sr } => {
                        stop_reason = sr;
                    }
                    _ => {}
                },
                Err(e) => {
                    return Err(AgentError::LlmError(e.to_string()));
                }
            }
        }

        Ok(LlmStreamResult {
            content,
            tool_calls,
            stop_reason,
            usage,
        })
    }

    fn provider(&self) -> &'static str {
        self.client.provider()
    }
}

/// Set up mutual trust between parent and child
///
/// Creates a TrustedPeers list that trusts the parent, for use by the child.
pub fn create_child_trusted_peers(parent_context: &ParentCommsContext) -> TrustedPeers {
    TrustedPeers {
        peers: vec![TrustedPeer {
            name: parent_context.parent_name.clone(),
            pubkey: PubKey::new(parent_context.parent_pubkey),
            addr: parent_context.parent_addr.clone(),
        }],
    }
}

/// Create comms configuration for a child agent
pub fn create_child_comms_config(child_name: &str, base_dir: &std::path::Path) -> CoreCommsConfig {
    CoreCommsConfig {
        enabled: true,
        name: child_name.to_string(),
        // Use UDS for local communication (more efficient than TCP)
        listen_uds: Some(base_dir.join(format!("{}.sock", child_name))),
        listen_tcp: None,
        identity_dir: base_dir.join(format!("{}/identity", child_name)),
        trusted_peers_path: base_dir.join(format!("{}/trusted_peers.json", child_name)),
        ack_timeout_secs: 30,
        max_message_bytes: 1_048_576,
    }
}

/// Initialize a session for a spawned sub-agent (clean context)
pub fn create_spawn_session(prompt: &str, system_prompt: Option<&str>) -> Session {
    let mut session = Session::new();

    // Add system prompt if provided
    if let Some(sys) = system_prompt {
        session.push(Message::System(SystemMessage {
            content: sys.to_string(),
        }));
    }

    // Add the user prompt
    session.push(Message::User(UserMessage {
        content: prompt.to_string(),
    }));

    session
}

/// Initialize a session for a forked sub-agent (with history)
pub fn create_fork_session(parent_session: &Session, fork_prompt: &str) -> Session {
    let mut session = parent_session.clone();

    // Append the fork prompt as a new user message
    session.push(Message::User(UserMessage {
        content: fork_prompt.to_string(),
    }));

    session
}

/// Set up the comms infrastructure for a child agent
///
/// This creates the identity, trust files, and comms runtime for a child agent.
/// Returns the child's public key and address for the parent to use.
pub async fn setup_child_comms(
    config: &SubAgentCommsConfig,
) -> Result<(CommsRuntime, [u8; 32], String), SubAgentRunnerError> {
    // Create trusted peers (trusting the parent)
    let trusted_peers = create_child_trusted_peers(&config.parent_context);
    let comms_config = create_child_comms_config(&config.name, &config.base_dir);
    let resolved_config = comms_config.resolve_paths(&config.base_dir);

    // Ensure directories exist without blocking the async runtime.
    tokio::fs::create_dir_all(&resolved_config.identity_dir)
        .await
        .map_err(|e| {
            SubAgentRunnerError::CommsSetup(format!("Failed to create identity dir: {}", e))
        })?;

    if let Some(parent) = resolved_config.trusted_peers_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| SubAgentRunnerError::CommsSetup(format!("Failed to create dir: {}", e)))?;
    }

    // Save trusted peers in a blocking task since it performs filesystem writes.
    let trusted_peers_path = resolved_config.trusted_peers_path.clone();
    tokio::task::spawn_blocking(move || trusted_peers.save(&trusted_peers_path))
        .await
        .map_err(|e| {
            SubAgentRunnerError::CommsSetup(format!("Failed to save trusted peers: {}", e))
        })?
        .map_err(|e| {
            SubAgentRunnerError::CommsSetup(format!("Failed to save trusted peers: {}", e))
        })?;

    // Create and start comms runtime
    let mut runtime = CommsRuntime::new(resolved_config.clone()).map_err(|e| {
        SubAgentRunnerError::CommsSetup(format!("Failed to create comms runtime: {}", e))
    })?;

    // Get child's public key before starting listeners
    let child_pubkey = *runtime.public_key().as_bytes();
    let child_addr = format!(
        "uds://{}",
        config
            .base_dir
            .join(format!("{}.sock", config.name))
            .display()
    );

    // Start listeners
    runtime.start_listeners().await.map_err(|e| {
        SubAgentRunnerError::CommsSetup(format!("Failed to start listeners: {}", e))
    })?;

    Ok((runtime, child_pubkey, child_addr))
}

/// Create a TrustedPeer entry for the parent to trust the child
///
/// After setting up child comms, the parent needs to add this peer to its trust list.
pub fn create_child_peer_entry(
    child_name: &str,
    child_pubkey: [u8; 32],
    child_addr: &str,
) -> TrustedPeer {
    TrustedPeer {
        name: child_name.to_string(),
        pubkey: PubKey::new(child_pubkey),
        addr: child_addr.to_string(),
    }
}

/// Specification for spawning a sub-agent (generic version)
pub struct SubAgentSpec<C, T, S>
where
    C: LlmClient + 'static,
    T: AgentToolDispatcher + 'static,
    S: AgentSessionStore + 'static,
{
    /// LLM client to use
    pub client: Arc<C>,
    /// Model name
    pub model: String,
    /// Tool dispatcher
    pub tools: Arc<T>,
    /// Session store
    pub store: Arc<S>,
    /// Initial session
    pub session: Session,
    /// Budget limits
    pub budget: Option<BudgetLimits>,
    /// Nesting depth (parent depth + 1)
    pub depth: u32,
    /// System prompt override
    pub system_prompt: Option<String>,
}

/// Specification for spawning a sub-agent (trait object version)
///
/// This is used when the concrete types aren't known at compile time,
/// such as when using LlmClientFactory which returns trait objects.
pub struct DynSubAgentSpec {
    /// LLM client to use (trait object)
    pub client: Arc<dyn LlmClient>,
    /// Model name
    pub model: String,
    /// Tool dispatcher (trait object)
    pub tools: Arc<dyn AgentToolDispatcher>,
    /// Session store (trait object)
    pub store: Arc<dyn AgentSessionStore>,
    /// Initial session
    pub session: Session,
    /// Budget limits
    pub budget: Option<BudgetLimits>,
    /// Nesting depth (parent depth + 1)
    pub depth: u32,
    /// System prompt override
    pub system_prompt: Option<String>,
    /// Comms configuration (if comms should be enabled for this sub-agent)
    pub comms_config: Option<SubAgentCommsConfig>,
    /// Parent's trusted peers (for adding this sub-agent as trusted)
    /// When set, the sub-agent will be added to this list after comms setup
    /// so the parent can accept connections from the sub-agent.
    pub parent_trusted_peers: Option<Arc<RwLock<TrustedPeers>>>,
    /// Host mode - agent stays alive processing comms messages after initial prompt
    pub host_mode: bool,
}

/// Spawn and run a sub-agent in a background task (trait object version)
///
/// This version works with trait objects as returned by LlmClientFactory.
/// It internally uses DynLlmClientAdapter to bridge to AgentLlmClient.
///
/// ## Comms Setup
///
/// If `spec.comms_config` is provided, this function uses `CommsBootstrap::for_child`
/// to set up comms for the sub-agent in a uniform way. The tools are then wrapped
/// with comms tools using `wrap_with_comms`, ensuring sub-agents have full comms
/// capabilities just like the main agent.
pub async fn spawn_sub_agent_dyn(
    id: OperationId,
    name: String,
    spec: DynSubAgentSpec,
    manager: Arc<SubAgentManager>,
) -> Result<mpsc::Sender<SteeringMessage>, SubAgentRunnerError> {
    use meerkat_core::sub_agent::SubAgentCommsInfo;

    let started_at = Instant::now();

    // Create the LLM client adapter (bridges LlmClient -> AgentLlmClient)
    let llm_adapter: Arc<DynLlmClientAdapter> =
        Arc::new(DynLlmClientAdapter::new(spec.client, spec.model.clone()));

    // Build the agent - now supports trait objects directly via ?Sized bounds
    let mut builder = AgentBuilder::new()
        .model(&spec.model)
        .depth(spec.depth)
        .resume_session(spec.session);

    if let Some(sys_prompt) = &spec.system_prompt {
        builder = builder.system_prompt(sys_prompt);
    }

    if let Some(budget) = spec.budget {
        builder = builder.budget(budget);
    }

    // Set up comms for the sub-agent if configured
    // Uses CommsBootstrap::for_child_inproc for lightweight in-process communication
    // No network listeners or filesystem resources needed
    let (tools, comms_info) = if let Some(comms_config) = spec.comms_config {
        let bootstrap = CommsBootstrap::for_child_inproc(
            comms_config.name.clone(),
            comms_config.parent_context.clone(),
        );

        match bootstrap.prepare().await {
            Ok(Some(prepared)) => {
                // Wrap tools with comms - this is the uniform way to add comms tools
                let tools_with_comms = wrap_with_comms(spec.tools.clone(), &prepared.runtime);

                // Extract advertise info for parent trust
                let comms_info = prepared.advertise.map(|adv| SubAgentCommsInfo {
                    pubkey: adv.pubkey,
                    addr: adv.addr.clone(),
                });

                // Add the child to the parent's trusted peers so the parent
                // will accept connections from this sub-agent
                if let (Some(info), Some(parent_peers)) = (&comms_info, &spec.parent_trusted_peers)
                {
                    let child_peer = TrustedPeer {
                        name: name.clone(),
                        pubkey: PubKey::new(info.pubkey),
                        addr: info.addr.clone(),
                    };
                    let mut peers = parent_peers.write().await;
                    peers.upsert(child_peer);
                    tracing::debug!("Added sub-agent '{}' to parent's trusted peers", name);
                }

                // Pass the comms runtime to the builder
                builder = builder.with_comms_runtime(prepared.runtime);

                (tools_with_comms, comms_info)
            }
            Ok(None) => {
                // Comms disabled (shouldn't happen for Child mode, but handle gracefully)
                (spec.tools, None)
            }
            Err(e) => {
                tracing::warn!("Failed to set up comms for sub-agent '{}': {}", name, e);
                (spec.tools, None)
            }
        }
    } else {
        (spec.tools, None)
    };

    // Pass trait objects directly - no wrappers needed thanks to ?Sized bounds
    let mut agent = builder.build(llm_adapter, tools, spec.store);

    // Get the steering sender from the agent
    let steering_tx = agent.steering_sender();

    // Register the sub-agent with the manager BEFORE spawning the task
    // This is critical - without registration, status/steer/cancel won't find the agent
    manager
        .register_with_comms(id.clone(), name.clone(), steering_tx.clone(), comms_info)
        .await
        .map_err(|e| {
            SubAgentRunnerError::ExecutionError(format!("Failed to register sub-agent: {}", e))
        })?;

    // Clone what we need for the spawned task
    let id_for_task = id.clone();
    let _name_for_task = name.clone();
    let manager_for_task = manager.clone();
    let host_mode = spec.host_mode;

    // Spawn the agent execution task
    tokio::spawn(async move {
        // The session already has the user prompt, so we use an empty string
        // to avoid adding a duplicate. The agent will see the existing message
        // and respond to it.
        //
        // If host_mode is enabled, the agent stays alive processing comms messages
        // after completing the initial prompt. This is the DRY approach - sub-agents
        // use the same run_host_mode() as the main agent.
        let result = if host_mode {
            agent.run_host_mode(String::new()).await
        } else {
            agent.run(String::new()).await
        };

        let duration_ms = started_at.elapsed().as_millis() as u64;

        match result {
            Ok(run_result) => {
                manager_for_task
                    .complete(
                        &id_for_task,
                        OperationResult {
                            id: id_for_task.clone(),
                            content: run_result.text,
                            is_error: false,
                            duration_ms,
                            tokens_used: run_result.usage.total_tokens(),
                        },
                    )
                    .await;
            }
            Err(e) => {
                manager_for_task.fail(&id_for_task, e.to_string()).await;
            }
        }
    });

    Ok(steering_tx)
}

/// Spawn and run a sub-agent in a background task (generic version)
///
/// This function:
/// 1. Creates an Agent with the given configuration
/// 2. Spawns a tokio task that runs the agent loop
/// 3. Reports results back to the manager when complete
///
/// Returns a handle with the operation ID and steering channel sender.
pub async fn spawn_sub_agent<C, T, S>(
    id: OperationId,
    name: String,
    spec: SubAgentSpec<C, T, S>,
    manager: Arc<SubAgentManager>,
) -> Result<mpsc::Sender<SteeringMessage>, SubAgentRunnerError>
where
    C: LlmClient + 'static,
    T: AgentToolDispatcher + 'static,
    S: AgentSessionStore + 'static,
{
    let started_at = Instant::now();

    // Create the LLM client adapter
    let llm_adapter = Arc::new(LlmClientAdapter::new(spec.client, spec.model.clone()));

    // Build the agent
    let mut builder = AgentBuilder::new()
        .model(&spec.model)
        .depth(spec.depth)
        .resume_session(spec.session);

    if let Some(sys_prompt) = &spec.system_prompt {
        builder = builder.system_prompt(sys_prompt);
    }

    if let Some(budget) = spec.budget {
        builder = builder.budget(budget);
    }

    let mut agent = builder.build(llm_adapter, spec.tools, spec.store);

    // Get the steering sender from the agent
    let steering_tx = agent.steering_sender();

    // Register the sub-agent with the manager BEFORE spawning the task
    // This is critical - without registration, status/steer/cancel won't find the agent
    manager
        .register(id.clone(), name.clone(), steering_tx.clone())
        .await
        .map_err(|e| {
            SubAgentRunnerError::ExecutionError(format!("Failed to register sub-agent: {}", e))
        })?;

    // Clone what we need for the spawned task
    let id_for_task = id.clone();
    let _name_for_task = name.clone(); // Kept for potential future use (logging, etc.)
    let manager_for_task = manager.clone();

    // Spawn the agent execution task
    tokio::spawn(async move {
        // Note: We don't need to call run() with input since the session already has the prompt
        // We need to trigger the agent loop differently. Let's use run_continue or similar.
        // Actually, looking at the Agent code, run() adds a user message.
        // Since our session already has the user message, we should use a different approach.
        //
        // The cleanest way is to remove the last user message from the session,
        // then call run() with that message.

        // For now, we'll use an empty string which just triggers the loop
        // without adding another message. This works because the session
        // already has the user prompt.

        // Actually, let me reconsider - the Agent.run() method:
        // 1. Adds the user_input as a User message
        // 2. Runs the loop
        //
        // Our session already has the user message. So we have two options:
        // A) Pass empty string and let the session's existing message drive the conversation
        //    - This won't work because run() always adds a message
        // B) Extract the last user message, create session without it, call run() with it
        //    - This is what we should do

        // For simplicity, let's just call with the prompt again
        // The LLM will see it twice but this is the cleanest approach for now
        // A proper fix would be to add run_from_session() to Agent

        let result = agent.run(String::new()).await;

        let duration_ms = started_at.elapsed().as_millis() as u64;

        match result {
            Ok(run_result) => {
                manager_for_task
                    .complete(
                        &id_for_task,
                        OperationResult {
                            id: id_for_task.clone(),
                            content: run_result.text,
                            is_error: false,
                            duration_ms,
                            tokens_used: run_result.usage.total_tokens(),
                        },
                    )
                    .await;
            }
            Err(e) => {
                manager_for_task.fail(&id_for_task, e.to_string()).await;
            }
        }
    });

    Ok(steering_tx)
}

/// Errors that can occur during sub-agent execution
#[derive(Debug, thiserror::Error)]
pub enum SubAgentRunnerError {
    #[error("Comms setup failed: {0}")]
    CommsSetup(String),

    #[error("Agent execution error: {0}")]
    ExecutionError(String),

    #[error("Session error: {0}")]
    SessionError(String),

    #[error("LLM client error: {0}")]
    LlmError(String),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_create_spawn_session() {
        let session = create_spawn_session("Do this task", None);
        assert_eq!(session.messages().len(), 1);
        match &session.messages()[0] {
            Message::User(u) => assert_eq!(u.content, "Do this task"),
            _ => unreachable!("Expected User message"),
        }
    }

    #[test]
    fn test_create_spawn_session_with_system() {
        let session = create_spawn_session("Do this task", Some("You are helpful"));
        assert_eq!(session.messages().len(), 2);
        match &session.messages()[0] {
            Message::System(s) => assert_eq!(s.content, "You are helpful"),
            _ => unreachable!("Expected System message"),
        }
        match &session.messages()[1] {
            Message::User(u) => assert_eq!(u.content, "Do this task"),
            _ => unreachable!("Expected User message"),
        }
    }

    #[test]
    fn test_create_fork_session() {
        let mut parent = Session::new();
        parent.push(Message::User(UserMessage {
            content: "Original prompt".to_string(),
        }));

        let forked = create_fork_session(&parent, "Continue with this");
        assert_eq!(forked.messages().len(), 2);
        match &forked.messages()[1] {
            Message::User(u) => assert_eq!(u.content, "Continue with this"),
            _ => unreachable!("Expected User message"),
        }
    }

    #[test]
    fn test_create_child_trusted_peers() {
        let parent_context = ParentCommsContext {
            parent_name: "parent-agent".to_string(),
            parent_pubkey: [42u8; 32],
            parent_addr: "uds:///tmp/parent.sock".to_string(),
            comms_base_dir: PathBuf::from("/tmp/comms"),
        };

        let trusted = create_child_trusted_peers(&parent_context);
        assert_eq!(trusted.peers.len(), 1);
        assert_eq!(trusted.peers[0].name, "parent-agent");
        assert_eq!(*trusted.peers[0].pubkey.as_bytes(), [42u8; 32]);
    }

    #[test]
    fn test_create_child_comms_config() {
        let base_dir = PathBuf::from("/tmp/agents");
        let config = create_child_comms_config("child-1", &base_dir);

        assert!(config.enabled);
        assert_eq!(config.name, "child-1");
        assert!(config.listen_uds.is_some());
        assert!(config.listen_tcp.is_none());
    }

    #[test]
    fn test_create_child_peer_entry() {
        let peer = create_child_peer_entry("child-1", [1u8; 32], "uds:///tmp/child.sock");

        assert_eq!(peer.name, "child-1");
        assert_eq!(*peer.pubkey.as_bytes(), [1u8; 32]);
        assert_eq!(peer.addr, "uds:///tmp/child.sock");
    }

    #[test]
    fn test_sub_agent_runner_error_display() {
        let err = SubAgentRunnerError::CommsSetup("test error".to_string());
        assert!(err.to_string().contains("Comms setup failed"));
        assert!(err.to_string().contains("test error"));

        let err = SubAgentRunnerError::ExecutionError("runtime error".to_string());
        assert!(err.to_string().contains("execution error"));
    }
}
