//! Sub-agent management for Meerkat
//!
//! Handles fork/spawn operations, steering queues, and result collection.

use crate::budget::BudgetLimits;
use crate::error::AgentError;
use crate::ops::{
    ConcurrencyLimits, ContextStrategy, ForkBudgetPolicy, OperationId, OperationResult,
    SteeringHandle, SteeringMessage, SteeringStatus, SubAgentState, ToolAccessPolicy,
};
use crate::session::Session;
use crate::types::{Message, ToolDef, UserMessage};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, mpsc, watch};
use uuid::Uuid;

const MAX_STEERING_QUEUE: usize = 256;
const MAX_COMPLETED_AGENTS: usize = 256;

/// Notification sent when a sub-agent completes
#[derive(Debug, Clone)]
pub struct SubAgentCompletion {
    /// Agent ID that completed
    pub agent_id: OperationId,
    /// Agent name
    pub agent_name: String,
    /// Whether it was an error
    pub is_error: bool,
    /// Brief summary of the result (truncated if needed)
    pub summary: String,
}

/// Comms metadata for a sub-agent (for parent to send messages)
#[derive(Debug, Clone)]
pub struct SubAgentCommsInfo {
    /// Child's public key (Ed25519)
    pub pubkey: [u8; 32],
    /// Address to reach the child (e.g., "uds:///tmp/child.sock")
    pub addr: String,
}

/// Information about a sub-agent (public view)
#[derive(Debug, Clone)]
pub struct SubAgentInfo {
    /// Operation ID for this sub-agent
    pub id: OperationId,
    /// Name/identifier for this sub-agent
    pub name: String,
    /// Current state
    pub state: SubAgentState,
    /// Nesting depth
    pub depth: u32,
    /// Duration running in milliseconds
    pub running_ms: u64,
    /// Number of pending steering messages
    pub pending_steering: usize,
    /// Result when completed
    pub result: Option<OperationResult>,
    /// Comms info for sending messages to this sub-agent
    pub comms: Option<SubAgentCommsInfo>,
}

/// A running sub-agent handle
#[derive(Debug)]
pub struct SubAgentHandle {
    /// Operation ID for this sub-agent
    pub id: OperationId,
    /// Name/identifier for this sub-agent
    pub name: String,
    /// Current state
    pub state: SubAgentState,
    /// Steering message queue
    pub steering_queue: VecDeque<SteeringMessage>,
    /// Next steering sequence number
    pub steering_seq: u64,
    /// When the sub-agent started
    pub started_at: Instant,
    /// Nesting depth (0 = top-level)
    pub depth: u32,
    /// Channel to send steering messages
    steering_tx: mpsc::Sender<SteeringMessage>,
    /// Result when completed
    pub result: Option<OperationResult>,
    /// Comms info for sending messages to this sub-agent
    pub comms: Option<SubAgentCommsInfo>,
}

impl SubAgentHandle {
    /// Queue a steering message for this sub-agent
    pub fn queue_steering(&mut self, content: String) -> SteeringHandle {
        let id = Uuid::now_v7();
        let seq = self.steering_seq;
        self.steering_seq += 1;

        let msg = SteeringMessage {
            id,
            seq,
            content,
            sent_at: std::time::SystemTime::now(),
        };

        if self.steering_queue.len() >= MAX_STEERING_QUEUE {
            self.steering_queue.pop_front();
        }
        self.steering_queue.push_back(msg);

        SteeringHandle {
            id,
            seq,
            status: SteeringStatus::Queued,
        }
    }
}

/// Manager for sub-agents
pub struct SubAgentManager {
    /// Running sub-agents indexed by operation ID
    agents: RwLock<HashMap<OperationId, SubAgentHandle>>,
    /// Concurrency limits
    pub limits: ConcurrencyLimits,
    /// Current depth (for nested sub-agents)
    current_depth: u32,
    /// Completed results waiting to be collected
    completed_results: Mutex<Vec<OperationResult>>,
    /// Recently completed sub-agents (bounded to avoid unbounded growth)
    completed_agents: Mutex<VecDeque<SubAgentInfo>>,
    /// Notification sender for sub-agent completions
    completion_tx: watch::Sender<Option<SubAgentCompletion>>,
    /// Notification receiver (clone this for listeners)
    completion_rx: watch::Receiver<Option<SubAgentCompletion>>,
}

impl SubAgentManager {
    /// Create a new sub-agent manager
    pub fn new(limits: ConcurrencyLimits, current_depth: u32) -> Self {
        let (completion_tx, completion_rx) = watch::channel(None);
        Self {
            agents: RwLock::new(HashMap::new()),
            limits,
            current_depth,
            completed_results: Mutex::new(Vec::new()),
            completed_agents: Mutex::new(VecDeque::new()),
            completion_tx,
            completion_rx,
        }
    }

    /// Get a receiver for sub-agent completion notifications
    ///
    /// The receiver will be notified whenever any sub-agent completes (success or failure).
    /// Use this to interrupt wait operations when sub-agents finish.
    pub fn completion_receiver(&self) -> watch::Receiver<Option<SubAgentCompletion>> {
        self.completion_rx.clone()
    }

    /// Check if we can spawn more sub-agents
    pub async fn can_spawn(&self) -> bool {
        let agents = self.agents.read().await;
        let running = agents
            .values()
            .filter(|a| a.state == SubAgentState::Running)
            .count();
        running < self.limits.max_concurrent_agents && self.current_depth < self.limits.max_depth
    }

    /// Apply ContextStrategy to create messages for sub-agent
    pub fn apply_context_strategy(
        &self,
        parent_session: &Session,
        strategy: &ContextStrategy,
    ) -> Vec<Message> {
        match strategy {
            ContextStrategy::FullHistory => {
                // Copy all messages from parent
                parent_session.messages().to_vec()
            }
            ContextStrategy::LastTurns(n) => {
                // Get last N turns (each turn = user + assistant)
                let messages = parent_session.messages();
                let turn_count = *n as usize * 2;
                if messages.len() <= turn_count {
                    messages.to_vec()
                } else {
                    // Keep system message if present, plus last N turns
                    let mut result = Vec::new();
                    if let Some(Message::System(sys)) = messages.first() {
                        result.push(Message::System(sys.clone()));
                    }
                    let start = messages.len().saturating_sub(turn_count);
                    result.extend(messages[start..].to_vec());
                    result
                }
            }
            ContextStrategy::Summary { max_tokens } => {
                // Take system prompt plus as many recent messages as fit within token budget
                // Estimate ~4 chars per token for rough approximation
                let chars_budget = (*max_tokens as usize) * 4;
                let mut result = Vec::new();
                let mut chars_used = 0usize;

                // Always include system prompt if present
                if let Some(Message::System(sys)) = parent_session.messages().first() {
                    chars_used += sys.content.len();
                    result.push(Message::System(sys.clone()));
                }

                // Add messages from the end until we hit the budget
                let non_system: Vec<_> = parent_session
                    .messages()
                    .iter()
                    .skip(1) // Skip system message
                    .collect();

                let mut to_include = Vec::new();
                for msg in non_system.iter().rev() {
                    let msg_len = match msg {
                        Message::User(u) => u.content.len(),
                        Message::Assistant(a) => a.content.len(),
                        Message::System(s) => s.content.len(),
                        Message::ToolResults { results } => {
                            results.iter().map(|r| r.content.len()).sum()
                        }
                    };
                    if chars_used + msg_len > chars_budget {
                        break;
                    }
                    chars_used += msg_len;
                    to_include.push((*msg).clone());
                }

                // Reverse to maintain chronological order
                to_include.reverse();
                result.extend(to_include);
                result
            }
            ContextStrategy::Custom { messages } => {
                // Use provided messages directly
                messages.clone()
            }
        }
    }

    /// Apply ToolAccessPolicy to filter tools
    pub fn apply_tool_access_policy(
        &self,
        all_tools: &[ToolDef],
        policy: &ToolAccessPolicy,
    ) -> Vec<ToolDef> {
        match policy {
            ToolAccessPolicy::Inherit => all_tools.to_vec(),
            ToolAccessPolicy::AllowList(allowed) => all_tools
                .iter()
                .filter(|t| allowed.contains(&t.name))
                .cloned()
                .collect(),
            ToolAccessPolicy::DenyList(denied) => all_tools
                .iter()
                .filter(|t| !denied.contains(&t.name))
                .cloned()
                .collect(),
        }
    }

    /// Allocate budget for fork branches
    pub fn allocate_fork_budget(
        &self,
        parent_remaining: u64,
        branch_count: usize,
        policy: &ForkBudgetPolicy,
    ) -> Vec<BudgetLimits> {
        let per_branch = match policy {
            ForkBudgetPolicy::Equal => {
                let tokens = parent_remaining / branch_count as u64;
                BudgetLimits {
                    max_tokens: Some(tokens),
                    max_duration: None,
                    max_tool_calls: None,
                }
            }
            ForkBudgetPolicy::Proportional => {
                // For proportional, we'd need weights - default to equal
                let tokens = parent_remaining / branch_count as u64;
                BudgetLimits {
                    max_tokens: Some(tokens),
                    max_duration: None,
                    max_tool_calls: None,
                }
            }
            ForkBudgetPolicy::Fixed(tokens) => BudgetLimits {
                max_tokens: Some(*tokens),
                max_duration: None,
                max_tool_calls: None,
            },
            ForkBudgetPolicy::Remaining => BudgetLimits {
                max_tokens: Some(parent_remaining),
                max_duration: None,
                max_tool_calls: None,
            },
        };

        vec![per_branch; branch_count]
    }

    /// Register a new sub-agent
    pub async fn register(
        &self,
        id: OperationId,
        name: String,
        steering_tx: mpsc::Sender<SteeringMessage>,
    ) -> Result<(), AgentError> {
        self.register_with_comms(id, name, steering_tx, None).await
    }

    /// Register a new sub-agent with comms info
    pub async fn register_with_comms(
        &self,
        id: OperationId,
        name: String,
        steering_tx: mpsc::Sender<SteeringMessage>,
        comms: Option<SubAgentCommsInfo>,
    ) -> Result<(), AgentError> {
        let mut agents = self.agents.write().await;

        if agents.len() >= self.limits.max_concurrent_agents {
            return Err(AgentError::SubAgentLimitExceeded {
                limit: self.limits.max_concurrent_agents,
            });
        }

        let handle = SubAgentHandle {
            id: id.clone(),
            name,
            state: SubAgentState::Running,
            steering_queue: VecDeque::new(),
            steering_seq: 0,
            started_at: Instant::now(),
            depth: self.current_depth + 1,
            steering_tx,
            result: None,
            comms,
        };

        agents.insert(id, handle);
        Ok(())
    }

    /// Send steering message to a running sub-agent
    pub async fn steer(
        &self,
        id: &OperationId,
        message: String,
    ) -> Result<SteeringHandle, AgentError> {
        let mut agents = self.agents.write().await;
        let handle = agents
            .get_mut(id)
            .ok_or_else(|| AgentError::SubAgentNotFound { id: id.to_string() })?;

        if handle.state != SubAgentState::Running {
            return Err(AgentError::SubAgentNotRunning {
                id: id.to_string(),
                state: format!("{:?}", handle.state),
            });
        }

        let steering_handle = handle.queue_steering(message.clone());

        // Also send through channel for immediate delivery
        let steering_msg = SteeringMessage {
            id: steering_handle.id,
            seq: steering_handle.seq,
            content: message,
            sent_at: std::time::SystemTime::now(),
        };

        let _ = handle.steering_tx.send(steering_msg).await;

        Ok(steering_handle)
    }

    /// Mark a sub-agent as completed
    pub async fn complete(&self, id: &OperationId, result: OperationResult) {
        let agent_name;
        let mut handle = {
            let mut agents = self.agents.write().await;
            agents.remove(id)
        };
        if let Some(ref mut handle) = handle {
            handle.state = SubAgentState::Completed;
            handle.result = Some(result.clone());
            agent_name = handle.name.clone();
        } else {
            agent_name = "unknown".to_string();
        }
        if let Some(handle) = handle {
            self.record_completed(handle, Some(result.duration_ms))
                .await;
        }

        // Add to completed results for collection
        {
            let mut completed = self.completed_results.lock().await;
            completed.push(result.clone());
        }

        // Notify listeners
        let summary = if result.content.len() > 200 {
            format!("{}...", &result.content[..200])
        } else {
            result.content.clone()
        };
        let _ = self.completion_tx.send(Some(SubAgentCompletion {
            agent_id: id.clone(),
            agent_name,
            is_error: result.is_error,
            summary,
        }));
    }

    /// Mark a sub-agent as failed
    pub async fn fail(&self, id: &OperationId, error: String) {
        let agent_name;
        let mut handle = {
            let mut agents = self.agents.write().await;
            agents.remove(id)
        };
        let duration_ms = handle
            .as_ref()
            .map(|h| h.started_at.elapsed().as_millis() as u64);
        if let Some(ref mut handle) = handle {
            handle.state = SubAgentState::Failed;
            handle.result = Some(OperationResult {
                id: id.clone(),
                content: error.clone(),
                is_error: true,
                duration_ms: duration_ms.unwrap_or_default(),
                tokens_used: 0,
            });
            agent_name = handle.name.clone();
        } else {
            agent_name = "unknown".to_string();
        }
        if let Some(handle) = handle {
            self.record_completed(handle, duration_ms).await;
        }

        // Notify listeners
        let _ = self.completion_tx.send(Some(SubAgentCompletion {
            agent_id: id.clone(),
            agent_name,
            is_error: true,
            summary: error,
        }));
    }

    /// Cancel a sub-agent
    pub async fn cancel(&self, id: &OperationId) {
        let mut handle = {
            let mut agents = self.agents.write().await;
            agents.remove(id)
        };
        let duration_ms = handle
            .as_ref()
            .map(|h| h.started_at.elapsed().as_millis() as u64);
        if let Some(ref mut handle) = handle {
            handle.state = SubAgentState::Cancelled;
        }
        if let Some(handle) = handle {
            self.record_completed(handle, duration_ms).await;
        }
    }

    /// Collect all completed results (called at turn boundaries)
    pub async fn collect_completed(&self) -> Vec<OperationResult> {
        let mut completed = self.completed_results.lock().await;
        std::mem::take(&mut *completed)
    }

    /// Get the state of a sub-agent
    pub async fn get_state(&self, id: &OperationId) -> Option<SubAgentState> {
        let state = {
            let agents = self.agents.read().await;
            agents.get(id).map(|h| h.state.clone())
        };
        if state.is_some() {
            return state;
        }
        let completed = self.completed_agents.lock().await;
        completed
            .iter()
            .find(|info| &info.id == id)
            .map(|info| info.state.clone())
    }

    /// Get detailed information about a sub-agent (for status queries)
    pub async fn get_agent_info(&self, id: &OperationId) -> Option<SubAgentInfo> {
        let info = {
            let agents = self.agents.read().await;
            agents.get(id).map(|handle| SubAgentInfo {
                id: handle.id.clone(),
                name: handle.name.clone(),
                state: handle.state.clone(),
                depth: handle.depth,
                running_ms: handle.started_at.elapsed().as_millis() as u64,
                pending_steering: handle.steering_queue.len(),
                result: handle.result.clone(),
                comms: handle.comms.clone(),
            })
        };
        if info.is_some() {
            return info;
        }

        let completed = self.completed_agents.lock().await;
        completed.iter().find(|info| &info.id == id).cloned()
    }

    /// Get all agent infos (for list queries)
    pub async fn list_agents(&self) -> Vec<SubAgentInfo> {
        let mut infos: Vec<SubAgentInfo> = {
            let agents = self.agents.read().await;
            agents
                .values()
                .map(|h| SubAgentInfo {
                    id: h.id.clone(),
                    name: h.name.clone(),
                    state: h.state.clone(),
                    depth: h.depth,
                    running_ms: h.started_at.elapsed().as_millis() as u64,
                    pending_steering: h.steering_queue.len(),
                    result: h.result.clone(),
                    comms: h.comms.clone(),
                })
                .collect()
        };

        let completed = self.completed_agents.lock().await;
        infos.extend(completed.iter().cloned());
        infos
    }

    /// Get all running sub-agent IDs
    pub async fn running_ids(&self) -> Vec<OperationId> {
        let agents = self.agents.read().await;
        agents
            .iter()
            .filter(|(_, h)| h.state == SubAgentState::Running)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Check if there are any running sub-agents
    pub async fn has_running(&self) -> bool {
        let agents = self.agents.read().await;
        agents.values().any(|h| h.state == SubAgentState::Running)
    }

    async fn record_completed(&self, handle: SubAgentHandle, duration_override: Option<u64>) {
        let running_ms =
            duration_override.unwrap_or_else(|| handle.started_at.elapsed().as_millis() as u64);
        let info = SubAgentInfo {
            id: handle.id,
            name: handle.name,
            state: handle.state,
            depth: handle.depth,
            running_ms,
            pending_steering: handle.steering_queue.len(),
            result: handle.result,
            comms: handle.comms,
        };

        let mut completed = self.completed_agents.lock().await;
        if completed.len() >= MAX_COMPLETED_AGENTS {
            completed.pop_front();
        }
        completed.push_back(info);
    }

    /// Wait for all sub-agents to complete
    pub async fn wait_all(&self) {
        while self.has_running().await {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

/// Apply steering messages to a session before an LLM call
pub fn inject_steering_messages(session: &mut Session, messages: Vec<SteeringMessage>) {
    for msg in messages {
        session.push(Message::User(UserMessage {
            content: format!("[STEERING] {}", msg.content),
        }));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::SystemMessage;

    #[test]
    fn test_context_strategy_full_history() {
        let manager = SubAgentManager::new(ConcurrencyLimits::default(), 0);
        let mut session = Session::new();
        session.push(Message::System(SystemMessage {
            content: "System".to_string(),
        }));
        session.push(Message::User(UserMessage {
            content: "User 1".to_string(),
        }));

        let messages = manager.apply_context_strategy(&session, &ContextStrategy::FullHistory);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_context_strategy_last_turns() {
        let manager = SubAgentManager::new(ConcurrencyLimits::default(), 0);
        let mut session = Session::new();
        session.push(Message::System(SystemMessage {
            content: "System".to_string(),
        }));
        // Add 4 turns (8 messages)
        for i in 0..4 {
            session.push(Message::User(UserMessage {
                content: format!("User {}", i),
            }));
            session.push(Message::User(UserMessage {
                content: format!("Assistant {}", i),
            }));
        }

        // Get last 2 turns = 4 messages + system = 5
        let messages = manager.apply_context_strategy(&session, &ContextStrategy::LastTurns(2));
        assert_eq!(messages.len(), 5); // system + 4 turn messages
    }

    #[test]
    fn test_tool_access_policy_inherit() {
        let manager = SubAgentManager::new(ConcurrencyLimits::default(), 0);
        let tools = vec![
            ToolDef {
                name: "tool1".to_string(),
                description: "".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}, "required": []}),
            },
            ToolDef {
                name: "tool2".to_string(),
                description: "".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}, "required": []}),
            },
        ];

        let filtered = manager.apply_tool_access_policy(&tools, &ToolAccessPolicy::Inherit);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_tool_access_policy_allow_list() {
        let manager = SubAgentManager::new(ConcurrencyLimits::default(), 0);
        let tools = vec![
            ToolDef {
                name: "tool1".to_string(),
                description: "".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}, "required": []}),
            },
            ToolDef {
                name: "tool2".to_string(),
                description: "".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}, "required": []}),
            },
        ];

        let filtered = manager.apply_tool_access_policy(
            &tools,
            &ToolAccessPolicy::AllowList(vec!["tool1".to_string()]),
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "tool1");
    }

    #[test]
    fn test_tool_access_policy_deny_list() {
        let manager = SubAgentManager::new(ConcurrencyLimits::default(), 0);
        let tools = vec![
            ToolDef {
                name: "tool1".to_string(),
                description: "".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}, "required": []}),
            },
            ToolDef {
                name: "tool2".to_string(),
                description: "".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}, "required": []}),
            },
        ];

        let filtered = manager.apply_tool_access_policy(
            &tools,
            &ToolAccessPolicy::DenyList(vec!["tool1".to_string()]),
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "tool2");
    }

    #[test]
    fn test_fork_budget_allocation_equal() {
        let manager = SubAgentManager::new(ConcurrencyLimits::default(), 0);
        let budgets = manager.allocate_fork_budget(1000, 4, &ForkBudgetPolicy::Equal);
        assert_eq!(budgets.len(), 4);
        assert_eq!(budgets[0].max_tokens, Some(250));
    }

    #[test]
    fn test_fork_budget_allocation_fixed() {
        let manager = SubAgentManager::new(ConcurrencyLimits::default(), 0);
        let budgets = manager.allocate_fork_budget(1000, 2, &ForkBudgetPolicy::Fixed(500));
        assert_eq!(budgets.len(), 2);
        assert_eq!(budgets[0].max_tokens, Some(500));
        assert_eq!(budgets[1].max_tokens, Some(500));
    }

    #[test]
    fn test_steering_message_injection() {
        let mut session = Session::new();
        let messages = vec![SteeringMessage {
            id: Uuid::now_v7(),
            seq: 0,
            content: "Focus on security".to_string(),
            sent_at: std::time::SystemTime::now(),
        }];

        inject_steering_messages(&mut session, messages);

        let last = session.messages().last().unwrap();
        match last {
            Message::User(u) => {
                assert!(u.content.starts_with("[STEERING]"));
                assert!(u.content.contains("Focus on security"));
            }
            _ => unreachable!("Expected User message"),
        }
    }
}
