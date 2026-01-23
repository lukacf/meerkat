//! SDK helper functions for quick agent creation
//!
//! These helpers provide a fluent API for creating and running agents
//! without needing to wire up all the components manually.

use crate::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, BudgetLimits,
    LlmClient, LlmEvent, LlmRequest, LlmStreamResult, Message, RetryPolicy, RunResult, Session,
    StopReason, ToolCall, ToolDef, ToolError, Usage,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

/// Create an agent builder configured for Anthropic
#[cfg(feature = "anthropic")]
pub fn with_anthropic(api_key: impl Into<String>) -> QuickBuilder<crate::AnthropicClient> {
    let client = crate::AnthropicClient::new(api_key.into());
    QuickBuilder::new(client, "claude-sonnet-4".to_string())
}

/// Create an agent builder configured for OpenAI
#[cfg(feature = "openai")]
pub fn with_openai(api_key: impl Into<String>) -> QuickBuilder<crate::OpenAiClient> {
    let client = crate::OpenAiClient::new(api_key.into());
    QuickBuilder::new(client, "gpt-4o".to_string())
}

/// Create an agent builder configured for Gemini
#[cfg(feature = "gemini")]
pub fn with_gemini(api_key: impl Into<String>) -> QuickBuilder<crate::GeminiClient> {
    let client = crate::GeminiClient::new(api_key.into());
    QuickBuilder::new(client, "gemini-2.0-flash-exp".to_string())
}

/// A simplified builder for quick agent creation
pub struct QuickBuilder<C: LlmClient + 'static> {
    client: Arc<C>,
    model: String,
    system_prompt: Option<String>,
    max_tokens: u32,
    budget: Option<BudgetLimits>,
    retry_policy: Option<RetryPolicy>,
    store_path: Option<PathBuf>,
    tools: Vec<ToolDef>,
}

impl<C: LlmClient + 'static> QuickBuilder<C> {
    /// Create a new quick builder
    pub fn new(client: C, default_model: String) -> Self {
        Self {
            client: Arc::new(client),
            model: default_model,
            system_prompt: None,
            max_tokens: 4096,
            budget: None,
            retry_policy: None,
            store_path: None,
            tools: Vec::new(),
        }
    }

    /// Set the model to use
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the model to use (alias for `model`)
    pub fn with_model(self, model: impl Into<String>) -> Self {
        self.model(model)
    }

    /// Set the system prompt
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the system prompt (alias for `system_prompt`)
    pub fn with_system_prompt(self, prompt: impl Into<String>) -> Self {
        self.system_prompt(prompt)
    }

    /// Set maximum tokens per turn
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }

    /// Set budget limits
    pub fn budget(mut self, limits: BudgetLimits) -> Self {
        self.budget = Some(limits);
        self
    }

    /// Set budget limits (alias for `budget`)
    pub fn with_budget(self, limits: BudgetLimits) -> Self {
        self.budget(limits)
    }

    /// Set retry policy
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = Some(policy);
        self
    }

    /// Set retry policy (alias for `retry_policy`)
    pub fn with_retry_policy(self, policy: RetryPolicy) -> Self {
        self.retry_policy(policy)
    }

    /// Set session store path
    pub fn store_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.store_path = Some(path.into());
        self
    }

    /// Add a tool definition
    pub fn tool(mut self, tool: ToolDef) -> Self {
        self.tools.push(tool);
        self
    }

    /// Add multiple tool definitions
    pub fn tools(mut self, tools: impl IntoIterator<Item = ToolDef>) -> Self {
        self.tools.extend(tools);
        self
    }

    /// Run the agent with the given prompt
    pub async fn run(self, prompt: impl Into<String>) -> Result<RunResult, AgentError> {
        let llm_adapter = Arc::new(QuickLlmAdapter::new(self.client, self.model.clone()));
        let tool_adapter = Arc::new(QuickToolDispatcher::new(self.tools));
        let store_adapter = Arc::new(MemorySessionStore::new());

        let mut builder = AgentBuilder::new()
            .model(&self.model)
            .max_tokens_per_turn(self.max_tokens);

        if let Some(sys_prompt) = &self.system_prompt {
            builder = builder.system_prompt(sys_prompt);
        }

        if let Some(budget) = self.budget {
            builder = builder.budget(budget);
        }

        if let Some(retry) = self.retry_policy {
            builder = builder.retry_policy(retry);
        }

        let mut agent = builder.build(llm_adapter, tool_adapter, store_adapter);
        agent.run(prompt.into()).await
    }
}

/// LLM adapter for the quick builder
struct QuickLlmAdapter<C: LlmClient> {
    client: Arc<C>,
    model: String,
}

impl<C: LlmClient> QuickLlmAdapter<C> {
    fn new(client: Arc<C>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl<C: LlmClient + 'static> AgentLlmClient for QuickLlmAdapter<C> {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&serde_json::Value>,
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
                    LlmEvent::ToolCallComplete { id, name, args } => {
                        tool_calls.push(ToolCall { id, name, args });
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

/// Tool dispatcher for the quick builder
struct QuickToolDispatcher {
    tools: Vec<ToolDef>,
}

impl QuickToolDispatcher {
    fn new(tools: Vec<ToolDef>) -> Self {
        Self { tools }
    }
}

#[async_trait]
impl AgentToolDispatcher for QuickToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        self.tools.clone()
    }

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
        // In the quick builder, tools are registered but need external dispatchers
        // For now, return an error - users can use the full builder for custom tool dispatch
        Err(ToolError::other(format!(
            "Tool '{}' is registered but no dispatcher is configured. \
             Use the full AgentBuilder for custom tool dispatch.",
            name
        )))
    }
}

/// In-memory session store for the quick builder
struct MemorySessionStore {
    sessions: std::sync::Mutex<std::collections::HashMap<String, Session>>,
}

impl MemorySessionStore {
    fn new() -> Self {
        Self {
            sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[async_trait]
impl AgentSessionStore for MemorySessionStore {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(session.id().to_string(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        let sessions = self.sessions.lock().unwrap();
        Ok(sessions.get(id).cloned())
    }
}

// Re-export builtin types for convenience
pub use meerkat_tools::{
    BuiltinToolConfig, CompositeDispatcher, CompositeDispatcherError, FileTaskStore, TaskStore,
    ensure_rkat_dir, find_project_root,
};

/// Create a tool dispatcher with built-in tools enabled.
///
/// This is a convenience function for setting up an agent with Meerkat's
/// built-in task management tools. It automatically:
/// - Detects the project root (using `.rkat/` or `.git/` markers)
/// - Creates the `.rkat/` directory if needed
/// - Sets up a file-based task store
/// - Creates a composite dispatcher with the configured built-in tools
///
/// # Arguments
/// * `config` - Configuration for enabling/disabling built-in tools
/// * `external` - Optional external dispatcher for additional tools (e.g., MCP router)
/// * `session_id` - Optional session ID for tracking tool usage
///
/// # Returns
/// An `Arc<CompositeDispatcher>` ready to use with `AgentBuilder::build()`
///
/// # Example
///
/// ```ignore
/// use meerkat::{
///     create_dispatcher_with_builtins, BuiltinToolConfig,
///     AgentBuilder, AnthropicClient,
/// };
/// use std::sync::Arc;
///
/// let dispatcher = create_dispatcher_with_builtins(
///     &BuiltinToolConfig::default(),
///     None,
///     Some("my-session".to_string()),
/// )?;
///
/// let agent = AgentBuilder::new()
///     .model("claude-sonnet-4")
///     .build(llm_client, dispatcher, store);
/// ```
pub fn create_dispatcher_with_builtins(
    config: &BuiltinToolConfig,
    external: Option<std::sync::Arc<dyn crate::AgentToolDispatcher>>,
    session_id: Option<String>,
) -> Result<std::sync::Arc<CompositeDispatcher>, CompositeDispatcherError> {
    let cwd = std::env::current_dir().map_err(CompositeDispatcherError::Io)?;
    let project_root = find_project_root(&cwd);
    ensure_rkat_dir(&project_root).map_err(CompositeDispatcherError::Io)?;
    let store = std::sync::Arc::new(FileTaskStore::in_project(&project_root));
    let dispatcher = CompositeDispatcher::new(store, config, external, session_id)?;
    Ok(std::sync::Arc::new(dispatcher))
}

/// Create a tool dispatcher with only built-in tools (no external tools).
///
/// This is a convenience wrapper around [`create_dispatcher_with_builtins`]
/// for the common case of using only built-in tools.
///
/// # Example
///
/// ```ignore
/// use meerkat::{create_builtins_dispatcher, BuiltinToolConfig, AgentBuilder};
///
/// let dispatcher = create_builtins_dispatcher(
///     &BuiltinToolConfig::default(),
///     Some("my-session".to_string()),
/// )?;
///
/// let agent = AgentBuilder::new()
///     .model("claude-sonnet-4")
///     .build(llm_client, dispatcher, store);
/// ```
pub fn create_builtins_dispatcher(
    config: &BuiltinToolConfig,
    session_id: Option<String>,
) -> Result<std::sync::Arc<CompositeDispatcher>, CompositeDispatcherError> {
    create_dispatcher_with_builtins(config, None, session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_tools::MemoryTaskStore;
    use std::sync::Arc;

    #[test]
    fn test_quick_builder_model() {
        // This test just verifies the builder compiles and has the right methods
        // Can't test the actual functionality without API keys
    }

    #[test]
    fn test_create_dispatcher_with_builtins() {
        // Use a memory store directly for testing (avoids file system)
        let store = Arc::new(MemoryTaskStore::new());
        let config = BuiltinToolConfig::default();
        let dispatcher =
            CompositeDispatcher::new(store, &config, None, Some("test-session".to_string()))
                .unwrap();

        // Should have all 4 task tools
        assert_eq!(dispatcher.builtin_count(), 4);
        assert!(dispatcher.is_builtin("task_create"));
        assert!(dispatcher.is_builtin("task_list"));
        assert!(dispatcher.is_builtin("task_get"));
        assert!(dispatcher.is_builtin("task_update"));
    }

    #[tokio::test]
    async fn test_builtin_tools_dispatch() {
        use crate::AgentToolDispatcher;

        let store = Arc::new(MemoryTaskStore::new());
        let config = BuiltinToolConfig::default();
        let dispatcher = CompositeDispatcher::new(store, &config, None, None).unwrap();

        // Create a task
        let args = serde_json::json!({
            "subject": "Integration test task",
            "description": "Testing the builtin dispatcher"
        });
        let result = dispatcher.dispatch("task_create", &args).await;
        assert!(result.is_ok());

        let task = result.unwrap();
        assert!(task.get("id").is_some());
        assert_eq!(task.get("subject").unwrap(), "Integration test task");

        // List tasks - returns an array directly
        let list_result = dispatcher
            .dispatch("task_list", &serde_json::json!({}))
            .await;
        assert!(list_result.is_ok());
        let list = list_result.unwrap();
        assert!(list.is_array());
        let tasks = list.as_array().unwrap();
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn test_create_dispatcher_in_project_dir() {
        // Test the actual helper function (uses tempdir to avoid polluting the workspace)
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        // Create a .rkat directory to mark it as a project
        std::fs::create_dir_all(temp_path.join(".rkat")).unwrap();

        // Change to temp dir for this test
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_path).unwrap();

        // Create dispatcher using the helper
        let result =
            create_builtins_dispatcher(&BuiltinToolConfig::default(), Some("test-123".to_string()));

        // Restore original dir
        std::env::set_current_dir(original_dir).unwrap();

        assert!(result.is_ok());
        let dispatcher = result.unwrap();
        assert_eq!(dispatcher.builtin_count(), 4);
    }

    #[tokio::test]
    async fn test_builtin_tools_in_project_dir() {
        use crate::AgentToolDispatcher;

        // Test using FileTaskStore in a temp directory
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        // Create the .rkat directory
        ensure_rkat_dir(temp_path).unwrap();

        // Create dispatcher with FileTaskStore
        let store = Arc::new(FileTaskStore::in_project(temp_path));
        let config = BuiltinToolConfig::default();
        let dispatcher =
            CompositeDispatcher::new(store, &config, None, Some("file-test-session".to_string()))
                .unwrap();

        // Create a task
        let create_result = dispatcher
            .dispatch(
                "task_create",
                &serde_json::json!({
                    "subject": "File store test",
                    "description": "Testing with real file storage"
                }),
            )
            .await;
        assert!(create_result.is_ok());

        let task = create_result.unwrap();
        let task_id = task.get("id").unwrap().as_str().unwrap();
        assert_eq!(task.get("created_by_session").unwrap(), "file-test-session");

        // Verify tasks.json was created in .rkat directory
        let tasks_file = temp_path.join(".rkat").join("tasks.json");
        assert!(tasks_file.exists(), "tasks.json should be created");

        // Get the task back
        let get_result = dispatcher
            .dispatch("task_get", &serde_json::json!({"id": task_id}))
            .await;
        assert!(get_result.is_ok());
        let retrieved = get_result.unwrap();
        assert_eq!(retrieved.get("subject").unwrap(), "File store test");
    }
}
