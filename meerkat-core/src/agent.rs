//! Agent - the core agent orchestrator
//!
//! The Agent struct ties together all components and runs the agent loop.

mod builder;
pub mod compact;
pub mod comms_impl;
mod extraction;
mod hook_impl;
mod runner;
mod state;

use crate::budget::Budget;
use crate::config::{AgentConfig, HookRunOverrides};
use crate::error::AgentError;
use crate::hooks::HookEngine;
use crate::retry::RetryPolicy;
use crate::schema::{CompiledSchema, SchemaError};
use crate::session::Session;
use crate::state::LoopState;
use crate::sub_agent::SubAgentManager;
use crate::types::{
    AssistantBlock, BlockAssistantMessage, Message, OutputSchema, StopReason, ToolCallView,
    ToolDef, ToolResult, Usage,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

pub use builder::AgentBuilder;
pub use runner::AgentRunner;

/// Special error prefix to signal tool calls that must be routed externally.
///
/// DEPRECATED: Use `ToolError::CallbackPending` or `AgentError::CallbackPending` instead.
/// This constant is kept for backward compatibility but will be removed in a future version.
#[deprecated(
    since = "0.2.0",
    note = "Use ToolError::CallbackPending or AgentError::CallbackPending instead"
)]
pub const CALLBACK_TOOL_PREFIX: &str = "CALLBACK_TOOL_PENDING:";

/// Trait for LLM clients that can be used with the agent
#[async_trait]
pub trait AgentLlmClient: Send + Sync {
    /// Stream a response from the LLM
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError>;

    /// Get the provider name
    fn provider(&self) -> &'static str;

    /// Compile an output schema for this provider.
    ///
    /// Default implementation normalizes the schema without provider-specific lowering.
    /// Adapters override this to apply provider-specific transformations (e.g.,
    /// Anthropic adds `additionalProperties: false`, Gemini strips unsupported keywords).
    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        // Default passthrough: normalized clone, no provider-specific lowering
        Ok(CompiledSchema {
            schema: output_schema.schema.as_value().clone(),
            warnings: Vec::new(),
        })
    }
}

/// Result of streaming from the LLM
pub struct LlmStreamResult {
    blocks: Vec<AssistantBlock>,
    stop_reason: StopReason,
    usage: Usage,
}

impl LlmStreamResult {
    pub fn new(blocks: Vec<AssistantBlock>, stop_reason: StopReason, usage: Usage) -> Self {
        Self {
            blocks,
            stop_reason,
            usage,
        }
    }

    pub fn blocks(&self) -> &[AssistantBlock] {
        &self.blocks
    }
    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason
    }
    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    pub fn into_message(self) -> BlockAssistantMessage {
        BlockAssistantMessage {
            blocks: self.blocks,
            stop_reason: self.stop_reason,
        }
    }

    pub fn into_parts(self) -> (Vec<AssistantBlock>, StopReason, Usage) {
        (self.blocks, self.stop_reason, self.usage)
    }
}

/// Trait for tool dispatchers
#[async_trait]
pub trait AgentToolDispatcher: Send + Sync {
    /// Get available tool definitions
    fn tools(&self) -> Arc<[Arc<ToolDef>]>;
    /// Execute a tool call
    async fn dispatch(&self, call: ToolCallView<'_>)
    -> Result<ToolResult, crate::error::ToolError>;
}

/// A tool dispatcher that filters tools based on a policy
///
/// Tools are filtered once at construction time based on the allowed_tools list.
/// The inner dispatcher is used for actual dispatch, but only allowed tools are
/// exposed via tools() and dispatch() returns AccessDenied for filtered tools.
pub struct FilteredToolDispatcher<T: AgentToolDispatcher + ?Sized> {
    inner: Arc<T>,
    allowed_tools: HashSet<String>,
    /// Pre-computed filtered tool list (computed once at construction)
    filtered_tools: Arc<[Arc<ToolDef>]>,
}

impl<T: AgentToolDispatcher + ?Sized> FilteredToolDispatcher<T> {
    pub fn new(inner: Arc<T>, allowed_tools: Vec<String>) -> Self {
        let allowed_set: HashSet<String> = allowed_tools.into_iter().collect();

        // Filter tools once at construction - the tool registry is static for agent lifetime
        let inner_tools = inner.tools();
        let filtered: Vec<Arc<ToolDef>> = inner_tools
            .iter()
            .filter(|t| allowed_set.contains(t.name.as_str()))
            .map(Arc::clone)
            .collect();

        Self {
            inner,
            allowed_tools: allowed_set,
            filtered_tools: filtered.into(),
        }
    }
}

#[async_trait]
impl<T: AgentToolDispatcher + ?Sized + 'static> AgentToolDispatcher for FilteredToolDispatcher<T> {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.filtered_tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<ToolResult, crate::error::ToolError> {
        if !self.allowed_tools.contains(call.name) {
            return Err(crate::error::ToolError::access_denied(call.name));
        }
        self.inner.dispatch(call).await
    }
}

/// Trait for session stores
#[async_trait]
pub trait AgentSessionStore: Send + Sync {
    async fn save(&self, session: &Session) -> Result<(), AgentError>;
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError>;
}

/// Trait for comms runtime that can be used with the agent
#[async_trait]
pub trait CommsRuntime: Send + Sync {
    /// Drain comms inbox and return messages formatted for the LLM
    async fn drain_messages(&self) -> Vec<String>;
    /// Get a notification when new messages arrive
    fn inbox_notify(&self) -> Arc<tokio::sync::Notify>;
}

/// The main Agent struct
pub struct Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized,
    T: AgentToolDispatcher + ?Sized,
    S: AgentSessionStore + ?Sized,
{
    config: AgentConfig,
    client: Arc<C>,
    tools: Arc<T>,
    store: Arc<S>,
    session: Session,
    budget: Budget,
    retry_policy: RetryPolicy,
    state: LoopState,
    sub_agent_manager: Arc<SubAgentManager>,
    depth: u32,
    pub(super) comms_runtime: Option<Arc<dyn CommsRuntime>>,
    pub(super) hook_engine: Option<Arc<dyn HookEngine>>,
    pub(super) hook_run_overrides: HookRunOverrides,
    /// Optional context compaction strategy.
    pub(crate) compactor: Option<Arc<dyn crate::compact::Compactor>>,
    /// Input tokens from the last LLM response (for compaction trigger).
    pub(crate) last_input_tokens: u64,
    /// Turn number when compaction last occurred.
    pub(crate) last_compaction_turn: Option<u32>,
}
