//! Composite dispatcher that combines multiple dispatchers into one.

#[cfg(not(target_arch = "wasm32"))]
use crate::builtin::shell::{JobManager, ShellConfig};
#[cfg(feature = "skills")]
use crate::builtin::skills::SkillToolSet;
use crate::builtin::store::TaskStore;
use crate::builtin::{BuiltinTool, BuiltinToolConfig, BuiltinToolError, ToolOutput};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ExternalToolUpdate;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
use serde_json::Value;
use std::collections::HashSet;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::Arc;

/// Error returned by the composite dispatcher.
#[derive(Debug, thiserror::Error)]
pub enum CompositeDispatcherError {
    #[error("Builtin tool error: {0}")]
    Builtin(#[from] BuiltinToolError),
    #[error("Tool collision: name '{0}' is already registered")]
    Collision(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Tool initialization failed for '{name}': {message}")]
    ToolInitFailed { name: String, message: String },
}

/// A composite dispatcher that combines multiple sources of tools.
pub struct CompositeDispatcher {
    builtin_tools: Vec<Arc<dyn BuiltinTool>>,
    #[cfg(feature = "skills")]
    skill_tools: Option<SkillToolSet>,
    external: Option<Arc<dyn AgentToolDispatcher>>,
    #[allow(dead_code)]
    task_store: Arc<dyn TaskStore>,
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(dead_code)]
    job_manager: Option<Arc<JobManager>>,
    allowed_tools: HashSet<String>,
}

impl CompositeDispatcher {
    /// Create a new composite dispatcher with builtin tools.
    ///
    /// `image_tool_results` is accepted for API compatibility but no longer used
    /// for filtering — view_image is always registered. Visibility is controlled
    /// at the factory level via `ToolScope` external filters.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(
        task_store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        project_root: Option<PathBuf>,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
        _image_tool_results: bool,
    ) -> Result<Self, CompositeDispatcherError> {
        let mut builtin_tools: Vec<Arc<dyn BuiltinTool>> = Vec::new();
        let shell_session_id = session_id.clone();
        let project_root = project_root
            .or_else(|| shell_config.as_ref().map(|cfg| cfg.project_root.clone()))
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| CompositeDispatcherError::ToolInitFailed {
                name: "apply_patch".to_string(),
                message: "failed to resolve project root".to_string(),
            })?;

        // Add task tools
        use crate::builtin::tasks::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
        builtin_tools.push(Arc::new(TaskListTool::new(task_store.clone())));
        builtin_tools.push(Arc::new(TaskGetTool::new(task_store.clone())));
        builtin_tools.push(Arc::new(TaskCreateTool::with_session_opt(
            task_store.clone(),
            session_id.clone(),
        )));
        builtin_tools.push(Arc::new(TaskUpdateTool::with_session_opt(
            task_store.clone(),
            session_id,
        )));

        // Add utility tools
        use crate::builtin::utility::{ApplyPatchTool, DateTimeTool, ViewImageTool, WaitTool};
        builtin_tools.push(Arc::new(WaitTool::new()));
        builtin_tools.push(Arc::new(DateTimeTool::new()));
        builtin_tools.push(Arc::new(ApplyPatchTool::new(project_root.clone())));
        builtin_tools.push(Arc::new(ViewImageTool::new(project_root)));

        // Add shell tools if enabled
        let job_manager = if let Some(cfg) = shell_config {
            if cfg.enabled {
                let mut manager = JobManager::new(cfg.clone());
                if let Some(session_id) = shell_session_id
                    .as_deref()
                    .and_then(|id| meerkat_core::types::SessionId::parse(id).ok())
                {
                    manager = manager.with_owner_session_id(session_id);
                }
                let mgr = Arc::new(manager);
                use crate::builtin::shell::{
                    ShellJobCancelTool, ShellJobStatusTool, ShellJobsListTool, ShellTool,
                };
                // Use with_job_manager to share the same JobManager between ShellTool
                // and job control tools. This ensures background jobs spawned via
                // ShellTool are visible to shell_jobs/shell_job_status/shell_job_cancel.
                builtin_tools.push(Arc::new(ShellTool::with_job_manager(cfg, mgr.clone())));
                builtin_tools.push(Arc::new(ShellJobStatusTool::new(mgr.clone())));
                builtin_tools.push(Arc::new(ShellJobsListTool::new(mgr.clone())));
                builtin_tools.push(Arc::new(ShellJobCancelTool::new(mgr.clone())));
                Some(mgr)
            } else {
                None
            }
        } else {
            None
        };

        let mut allowed_tools = HashSet::new();
        let resolved_policy = config.resolve();
        for tool in &builtin_tools {
            let name = tool.name().to_string();
            if resolved_policy.is_enabled(&name, tool.default_enabled()) {
                allowed_tools.insert(name);
            }
        }

        // NOTE: view_image is always kept in allowed_tools regardless of image_tool_results.
        // Visibility gating for non-image models is handled at the factory level via
        // ToolScope external filters, enabling hot-swap to reveal view_image later.

        Ok(Self {
            builtin_tools,
            #[cfg(feature = "skills")]
            skill_tools: None,
            external,
            task_store,
            job_manager,
            allowed_tools,
        })
    }

    /// Create a new composite dispatcher with builtin tools (wasm32 version, no shell).
    #[cfg(target_arch = "wasm32")]
    pub fn new_wasm(
        task_store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
    ) -> Result<Self, CompositeDispatcherError> {
        let mut builtin_tools: Vec<Arc<dyn BuiltinTool>> = Vec::new();

        // Add task tools
        use crate::builtin::tasks::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
        builtin_tools.push(Arc::new(TaskListTool::new(task_store.clone())));
        builtin_tools.push(Arc::new(TaskGetTool::new(task_store.clone())));
        builtin_tools.push(Arc::new(TaskCreateTool::with_session_opt(
            task_store.clone(),
            session_id.clone(),
        )));
        builtin_tools.push(Arc::new(TaskUpdateTool::with_session_opt(
            task_store.clone(),
            session_id,
        )));

        // Add utility tools
        use crate::builtin::utility::{DateTimeTool, WaitTool};
        builtin_tools.push(Arc::new(WaitTool::new()));
        builtin_tools.push(Arc::new(DateTimeTool::new()));

        let mut allowed_tools = HashSet::new();
        let resolved_policy = config.resolve();
        for tool in &builtin_tools {
            let name = tool.name().to_string();
            if resolved_policy.is_enabled(&name, tool.default_enabled()) {
                allowed_tools.insert(name);
            }
        }

        Ok(Self {
            builtin_tools,
            #[cfg(feature = "skills")]
            skill_tools: None,
            external,
            task_store,
            allowed_tools,
        })
    }

    /// Register skill discovery tools (browse_skills, load_skill).
    #[cfg(feature = "skills")]
    pub fn register_skill_tools(&mut self, tool_set: SkillToolSet) {
        for tool in tool_set.tools() {
            self.allowed_tools.insert(tool.name().to_string());
        }
        self.skill_tools = Some(tool_set);
    }

    /// Get usage instructions for all enabled tools.
    pub fn usage_instructions(&self) -> String {
        let mut out = String::from("# Available Tools\n\n");
        let mut seen_names: HashSet<String> = HashSet::new();
        for tool in &self.builtin_tools {
            if self.allowed_tools.contains(tool.name()) {
                seen_names.insert(tool.name().to_string());
                {
                    use std::fmt::Write;
                    let _ = write!(out, "## {}\n{}\n\n", tool.name(), tool.def().description);
                }
            }
        }
        if let Some(ref ext) = self.external {
            let mut wrote_external_header = false;
            for tool in ext.tools().iter() {
                if seen_names.contains(&tool.name) {
                    continue;
                }
                if !wrote_external_header {
                    out.push_str(
                        "## External tools\nProvided by integrated runtimes/services.\n\n",
                    );
                    wrote_external_header = true;
                }
                seen_names.insert(tool.name.clone());
                {
                    use std::fmt::Write;
                    let _ = write!(out, "## {}\n{}\n\n", tool.name, tool.description);
                }
            }
        }
        out
    }

    /// Return the shared shell job manager when shell tools are enabled.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn shell_job_manager(&self) -> Option<Arc<JobManager>> {
        self.job_manager.clone()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for CompositeDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut tools = Vec::new();
        let mut seen_names = HashSet::new();

        // Add allowed builtin tools first (they take precedence)
        for tool in &self.builtin_tools {
            if self.allowed_tools.contains(tool.name()) {
                seen_names.insert(tool.name().to_string());
                tools.push(Arc::new(tool.def()));
            }
        }

        // Add skill tools
        #[cfg(feature = "skills")]
        if let Some(ref skill) = self.skill_tools {
            for tool in skill.tools() {
                if self.allowed_tools.contains(tool.name()) {
                    seen_names.insert(tool.name().to_string());
                    tools.push(Arc::new(tool.def()));
                }
            }
        }

        // Add external tools, skipping duplicates to avoid LLM confusion
        if let Some(ref ext) = self.external {
            for tool in ext.tools().iter() {
                if !seen_names.contains(&tool.name) {
                    seen_names.insert(tool.name.clone());
                    tools.push(Arc::clone(tool));
                }
                // Note: duplicates are silently skipped; builtins take precedence
            }
        }

        tools.into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let args: Value =
            serde_json::from_str(call.args.get()).map_err(|e| ToolError::InvalidArguments {
                name: call.name.to_string(),
                reason: e.to_string(),
            })?;
        // First check if it's an allowed tool
        if !self.allowed_tools.contains(call.name) {
            // Check external dispatcher for non-allowed tools
            if let Some(ref ext) = self.external
                && ext.tools().iter().any(|t| t.name == call.name)
            {
                return ext.dispatch(call).await;
            }
            return Err(ToolError::NotFound {
                name: call.name.to_string(),
            });
        }

        // Check builtin tools
        for tool in &self.builtin_tools {
            if tool.name() == call.name {
                let output = tool.call(args.clone()).await.map_err(|e| match e {
                    BuiltinToolError::InvalidArgs(msg) => ToolError::InvalidArguments {
                        name: call.name.to_string(),
                        reason: msg,
                    },
                    BuiltinToolError::ExecutionFailed(msg) => {
                        ToolError::ExecutionFailed { message: msg }
                    }
                    BuiltinToolError::TaskError(te) => ToolError::ExecutionFailed { message: te },
                })?;
                let operation_ids = tool.operation_ids_for_output(&output);
                match output {
                    ToolOutput::Json(value) => {
                        let content = match &value {
                            Value::String(s) => s.clone(),
                            _ => serde_json::to_string(&value).unwrap_or_default(),
                        };
                        return Ok(ToolResult::new(call.id.to_string(), content, false)
                            .with_operation_ids(operation_ids));
                    }
                    ToolOutput::Blocks(blocks) => {
                        return Ok(ToolResult::with_blocks(call.id.to_string(), blocks, false)
                            .with_operation_ids(operation_ids));
                    }
                }
            }
        }

        // Check skill tools
        #[cfg(feature = "skills")]
        if let Some(ref skill) = self.skill_tools {
            for tool in skill.tools() {
                if tool.name() == call.name {
                    let output = tool.call(args.clone()).await.map_err(|e| match e {
                        BuiltinToolError::InvalidArgs(msg) => ToolError::InvalidArguments {
                            name: call.name.to_string(),
                            reason: msg,
                        },
                        BuiltinToolError::ExecutionFailed(msg) => {
                            ToolError::ExecutionFailed { message: msg }
                        }
                        BuiltinToolError::TaskError(te) => {
                            ToolError::ExecutionFailed { message: te }
                        }
                    })?;
                    let operation_ids = tool.operation_ids_for_output(&output);
                    match output {
                        ToolOutput::Json(value) => {
                            let content = match &value {
                                Value::String(s) => s.clone(),
                                _ => serde_json::to_string(&value).unwrap_or_default(),
                            };
                            return Ok(ToolResult::new(call.id.to_string(), content, false)
                                .with_operation_ids(operation_ids));
                        }
                        ToolOutput::Blocks(blocks) => {
                            return Ok(ToolResult::with_blocks(call.id.to_string(), blocks, false)
                                .with_operation_ids(operation_ids));
                        }
                    }
                }
            }
        }

        Err(ToolError::NotFound {
            name: call.name.to_string(),
        })
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        if let Some(ref ext) = self.external {
            ext.poll_external_updates().await
        } else {
            ExternalToolUpdate::default()
        }
    }

    fn bind_wait_interrupt(
        self: Arc<Self>,
        rx: meerkat_core::wait_interrupt::WaitInterruptReceiver,
    ) -> Result<Arc<dyn AgentToolDispatcher>, meerkat_core::wait_interrupt::WaitInterruptBindError>
    {
        let mut owned = Arc::try_unwrap(self)
            .map_err(|_| meerkat_core::wait_interrupt::WaitInterruptBindError::SharedOwnership)?;
        // Swap the wait tool with an interrupt-aware version
        use crate::builtin::utility::WaitTool;
        let new_wait = Arc::new(WaitTool::with_interrupt(rx));
        for tool in &mut owned.builtin_tools {
            if tool.name() == "wait" {
                *tool = new_wait;
                break;
            }
        }
        Ok(Arc::new(owned))
    }

    fn supports_wait_interrupt(&self) -> bool {
        self.builtin_tools.iter().any(|t| t.name() == "wait")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::MemoryTaskStore;
    use serde_json::json;

    struct MockExternalDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    impl MockExternalDispatcher {
        fn new(name: &str, description: &str) -> Self {
            let tools: Arc<[Arc<ToolDef>]> = Arc::from([Arc::new(ToolDef {
                name: name.to_string(),
                description: description.to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            })]);
            Self { tools }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MockExternalDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tools.clone()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            if self.tools.iter().any(|tool| tool.name == call.name) {
                return Ok(ToolResult::new(
                    call.id.to_string(),
                    "{}".to_string(),
                    false,
                ));
            }
            Err(ToolError::not_found(call.name))
        }
    }

    #[test]
    fn usage_instructions_include_external_tools() {
        let store = Arc::new(MemoryTaskStore::new());
        let external: Arc<dyn AgentToolDispatcher> =
            Arc::new(MockExternalDispatcher::new("mob_list", "List active mobs"));

        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            None,
            None,
            Some(external),
            None,
            true,
        )
        .expect("composite dispatcher should build");

        let usage = dispatcher.usage_instructions();
        assert!(usage.contains("External tools"));
        assert!(usage.contains("mob_list"));
        assert!(usage.contains("List active mobs"));
    }

    // set_wait_interrupt test removed — legacy API replaced by bind_wait_interrupt()

    #[tokio::test]
    async fn bind_wait_interrupt_swaps_in_interrupt_aware_wait_tool() {
        use crate::builtin::utility::WaitInterrupt;
        use meerkat_core::time_compat::Duration;

        let store = Arc::new(MemoryTaskStore::new());
        let dispatcher = Arc::new(
            CompositeDispatcher::new(
                store,
                &BuiltinToolConfig::default(),
                None,
                None,
                None,
                None,
                true,
            )
            .expect("composite dispatcher should build"),
        );

        let (tx, rx) = tokio::sync::watch::channel(None::<WaitInterrupt>);
        let rebound = dispatcher
            .bind_wait_interrupt(rx)
            .expect("bind_wait_interrupt should succeed");

        // Dispatch a wait call and interrupt it
        let call_json =
            serde_json::value::RawValue::from_string(r#"{"seconds": 30.0}"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "test-id",
            name: "wait",
            args: &call_json,
        };

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(Some(WaitInterrupt {
                reason: "bind test interrupt".to_string(),
            }));
        });

        let start = std::time::Instant::now();
        let result = rebound
            .dispatch(call)
            .await
            .expect("dispatch should succeed");
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "wait should be interrupted quickly, took {elapsed:?}"
        );
        let content: serde_json::Value = serde_json::from_str(&result.text_content()).unwrap();
        assert_eq!(content["status"], "interrupted");
    }

    #[test]
    #[allow(clippy::panic)]
    fn bind_wait_interrupt_shared_ownership_error() {
        let store = Arc::new(MemoryTaskStore::new());
        let dispatcher = Arc::new(
            CompositeDispatcher::new(
                store,
                &BuiltinToolConfig::default(),
                None,
                None,
                None,
                None,
                true,
            )
            .expect("composite dispatcher should build"),
        );
        let _clone = Arc::clone(&dispatcher);

        let (_tx, rx) =
            tokio::sync::watch::channel(None::<meerkat_core::wait_interrupt::WaitInterrupt>);
        match dispatcher.bind_wait_interrupt(rx) {
            Err(meerkat_core::wait_interrupt::WaitInterruptBindError::SharedOwnership) => {}
            Ok(_) => panic!("expected SharedOwnership error, got Ok"),
            Err(e) => panic!("expected SharedOwnership, got {e:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_json_string_produces_text_result() {
        let store = Arc::new(MemoryTaskStore::new());
        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("composite dispatcher should build");

        // The "wait" tool returns ToolOutput::Json with a JSON object.
        // A string JSON value should be returned as-is (not double-quoted).
        let call_json =
            serde_json::value::RawValue::from_string(r#"{"seconds": 0.1}"#.to_string()).unwrap();
        let call = ToolCallView {
            id: "test-str",
            name: "wait",
            args: &call_json,
        };
        let result = dispatcher
            .dispatch(call)
            .await
            .expect("dispatch should succeed");
        assert!(!result.is_error);
        // wait returns {"waited_seconds": ..., "status": "complete"} which is an object
        let parsed: serde_json::Value =
            serde_json::from_str(&result.text_content()).expect("content should be valid JSON");
        assert_eq!(parsed["status"], "complete");
    }

    #[tokio::test]
    async fn dispatch_json_object_produces_serialized_text() {
        let store = Arc::new(MemoryTaskStore::new());
        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("composite dispatcher should build");

        // datetime returns a JSON object - verify it's serialized to text
        let call_json = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "test-obj",
            name: "datetime",
            args: &call_json,
        };
        let result = dispatcher
            .dispatch(call)
            .await
            .expect("dispatch should succeed");
        assert!(!result.is_error);
        let parsed: serde_json::Value =
            serde_json::from_str(&result.text_content()).expect("content should be valid JSON");
        assert!(
            parsed.get("iso8601").is_some(),
            "should contain iso8601 field"
        );
    }

    #[tokio::test]
    async fn existing_datetime_tool_returns_json_output() {
        use crate::builtin::BuiltinTool;
        use crate::builtin::utility::DateTimeTool;

        let tool = DateTimeTool::new();
        let output = tool.call(json!({})).await.expect("call should succeed");

        // Verify it returns ToolOutput::Json
        let value = output.into_json().expect("should be Json variant");
        assert!(value.get("iso8601").is_some());
        assert!(value.get("unix_timestamp").is_some());
    }

    #[test]
    fn view_image_always_in_base_tool_set() {
        // view_image is always registered regardless of image_tool_results flag.
        // Visibility gating is handled at the factory/ToolScope level via external filters.
        for image_tool_results in [false, true] {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher = CompositeDispatcher::new(
                store,
                &BuiltinToolConfig::default(),
                None,
                None,
                None,
                None,
                image_tool_results,
            )
            .expect("composite dispatcher should build");

            let tools = dispatcher.tools();
            let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
            assert!(
                tool_names.contains(&"view_image".to_string()),
                "view_image should always be in base tool set (image_tool_results={image_tool_results}), but found: {tool_names:?}"
            );
        }
    }
}
