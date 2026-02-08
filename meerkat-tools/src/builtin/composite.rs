//! Composite dispatcher that combines multiple dispatchers into one.

use crate::builtin::shell::{JobManager, ShellConfig};
use crate::builtin::store::TaskStore;
#[cfg(feature = "sub-agents")]
use crate::builtin::sub_agent::SubAgentToolSet;
use crate::builtin::{BuiltinTool, BuiltinToolConfig, BuiltinToolError};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
use serde_json::Value;
use std::collections::HashSet;
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
    #[cfg(feature = "sub-agents")]
    sub_agent_tools: Option<SubAgentToolSet>,
    external: Option<Arc<dyn AgentToolDispatcher>>,
    #[allow(dead_code)]
    task_store: Arc<dyn TaskStore>,
    #[allow(dead_code)]
    job_manager: Option<Arc<JobManager>>,
    allowed_tools: HashSet<String>,
}

impl CompositeDispatcher {
    /// Create a new composite dispatcher with builtin tools.
    pub fn new(
        task_store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
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

        // Add shell tools if enabled
        let job_manager = if let Some(cfg) = shell_config {
            if cfg.enabled {
                let mgr = Arc::new(JobManager::new(cfg.clone()));
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

        Ok(Self {
            builtin_tools,
            #[cfg(feature = "sub-agents")]
            sub_agent_tools: None,
            external,
            task_store,
            job_manager,
            allowed_tools,
        })
    }

    /// Register sub-agent tools.
    #[cfg(feature = "sub-agents")]
    pub fn register_sub_agent_tools(
        &mut self,
        tool_set: SubAgentToolSet,
        config: &BuiltinToolConfig,
    ) -> Result<(), CompositeDispatcherError> {
        let resolved_policy = config.resolve();
        let names = [
            "agent_spawn",
            "agent_fork",
            "agent_status",
            "agent_cancel",
            "agent_list",
        ];
        for name in names {
            let name_str = name.to_string();
            if resolved_policy.is_enabled(&name_str, true) {
                self.allowed_tools.insert(name_str);
            }
        }
        self.sub_agent_tools = Some(tool_set);
        Ok(())
    }

    /// Get usage instructions for all enabled tools.
    pub fn usage_instructions(&self) -> String {
        let mut out = String::from("# Available Tools\n\n");
        for tool in &self.builtin_tools {
            if self.allowed_tools.contains(tool.name()) {
                out.push_str(&format!(
                    "## {}\n{}\n\n",
                    tool.name(),
                    tool.def().description
                ));
            }
        }
        #[cfg(feature = "sub-agents")]
        if self.sub_agent_tools.is_some() {
            out.push_str("## Sub-agent tools\nManage hierarchical agents.\n\n");
        }
        out
    }
}

#[async_trait]
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

        // Add allowed sub-agent tools
        #[cfg(feature = "sub-agents")]
        if let Some(ref sub) = self.sub_agent_tools {
            for tool in sub.tools() {
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
            if let Some(ref ext) = self.external {
                if ext.tools().iter().any(|t| t.name == call.name) {
                    return ext.dispatch(call).await;
                }
            }
            return Err(ToolError::NotFound {
                name: call.name.to_string(),
            });
        }

        // Check builtin tools
        for tool in &self.builtin_tools {
            if tool.name() == call.name {
                let value = tool.call(args.clone()).await.map_err(|e| match e {
                    BuiltinToolError::InvalidArgs(msg) => ToolError::InvalidArguments {
                        name: call.name.to_string(),
                        reason: msg,
                    },
                    BuiltinToolError::ExecutionFailed(msg) => {
                        ToolError::ExecutionFailed { message: msg }
                    }
                    BuiltinToolError::TaskError(te) => ToolError::ExecutionFailed { message: te },
                })?;
                let content = match &value {
                    Value::String(s) => s.clone(),
                    _ => serde_json::to_string(&value).unwrap_or_default(),
                };
                return Ok(ToolResult {
                    tool_use_id: call.id.to_string(),
                    content,
                    is_error: false,
                });
            }
        }

        // Check sub-agent tools
        #[cfg(feature = "sub-agents")]
        if let Some(ref sub) = self.sub_agent_tools {
            for tool in sub.tools() {
                if tool.name() == call.name {
                    let value = tool.call(args.clone()).await.map_err(|e| match e {
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
                    let content = match &value {
                        Value::String(s) => s.clone(),
                        _ => serde_json::to_string(&value).unwrap_or_default(),
                    };
                    return Ok(ToolResult {
                        tool_use_id: call.id.to_string(),
                        content,
                        is_error: false,
                    });
                }
            }
        }

        Err(ToolError::NotFound {
            name: call.name.to_string(),
        })
    }
}
