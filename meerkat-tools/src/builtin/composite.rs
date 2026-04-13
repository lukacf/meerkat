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
use meerkat_core::agent::{BindOutcome, DispatcherCapabilities, OpsLifecycleBindError};
use meerkat_core::error::ToolError;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::types::{SessionId, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{ToolCatalogCapabilities, ToolCatalogEntry};
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
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    builtin_config: BuiltinToolConfig,
    #[allow(dead_code)]
    task_store: Arc<dyn TaskStore>,
    #[cfg(not(target_arch = "wasm32"))]
    project_root: Option<PathBuf>,
    #[cfg(not(target_arch = "wasm32"))]
    shell_config: Option<ShellConfig>,
    #[allow(dead_code)]
    session_id: Option<String>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    image_tool_results: bool,
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
        Self::new_with_ops_lifecycle(
            task_store,
            config,
            project_root,
            shell_config,
            external,
            session_id,
            None,
            _image_tool_results,
        )
    }

    /// Create a new composite dispatcher with an optional session-canonical ops registry.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_ops_lifecycle(
        task_store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        project_root: Option<PathBuf>,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
        ops_lifecycle: Option<Arc<dyn OpsLifecycleRegistry>>,
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
        use crate::builtin::utility::{ApplyPatchTool, DateTimeTool, ViewImageTool};
        builtin_tools.push(Arc::new(DateTimeTool::new()));
        builtin_tools.push(Arc::new(ApplyPatchTool::new(project_root.clone())));
        builtin_tools.push(Arc::new(ViewImageTool::new(project_root.clone())));

        // Add shell tools if enabled
        let job_manager = if let Some(ref cfg) = shell_config {
            if cfg.enabled {
                let mut manager = JobManager::new(cfg.clone());
                if let Some(session_id) = shell_session_id
                    .as_deref()
                    .and_then(|id| meerkat_core::types::SessionId::parse(id).ok())
                {
                    manager = manager.with_owner_session_id(session_id);
                }
                if let Some(registry) = ops_lifecycle {
                    manager = manager.with_ops_registry(registry);
                }
                let mgr = Arc::new(manager);
                use crate::builtin::shell::{
                    ShellJobCancelTool, ShellJobStatusTool, ShellJobsListTool, ShellTool,
                };
                // Use with_job_manager to share the same JobManager between ShellTool
                // and job control tools. This ensures background jobs spawned via
                // ShellTool are visible to shell_jobs/shell_job_status/shell_job_cancel.
                builtin_tools.push(Arc::new(ShellTool::with_job_manager(
                    cfg.clone(),
                    mgr.clone(),
                )));
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
            builtin_config: config.clone(),
            task_store,
            project_root: Some(project_root),
            shell_config,
            session_id: shell_session_id,
            image_tool_results: _image_tool_results,
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
            session_id.clone(),
        )));

        // Add utility tools
        use crate::builtin::utility::DateTimeTool;
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
            builtin_config: config.clone(),
            task_store,
            session_id,
            image_tool_results: true,
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

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: self
                .external
                .as_ref()
                .is_none_or(|dispatcher| dispatcher.tool_catalog_capabilities().exact_catalog),
            may_require_catalog_control_plane: self.external.as_ref().is_some_and(|dispatcher| {
                dispatcher
                    .tool_catalog_capabilities()
                    .may_require_catalog_control_plane
            }),
        }
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.external
            .as_ref()
            .map(|dispatcher| dispatcher.pending_catalog_sources())
            .unwrap_or_else(|| Arc::from([]))
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        let mut catalog = Vec::new();
        let mut seen_names = HashSet::new();

        for tool in &self.builtin_tools {
            if self.allowed_tools.contains(tool.name()) {
                seen_names.insert(tool.name().to_string());
                catalog.push(ToolCatalogEntry::session_inline(Arc::new(tool.def()), true));
            }
        }

        #[cfg(feature = "skills")]
        if let Some(ref skill) = self.skill_tools {
            for tool in skill.tools() {
                if self.allowed_tools.contains(tool.name())
                    && seen_names.insert(tool.name().to_string())
                {
                    catalog.push(ToolCatalogEntry::session_inline(Arc::new(tool.def()), true));
                }
            }
        }

        if let Some(ref ext) = self.external {
            if ext.tool_catalog_capabilities().exact_catalog {
                for entry in ext.tool_catalog().iter() {
                    if seen_names.insert(entry.tool.name.clone()) {
                        catalog.push(entry.clone());
                    }
                }
            } else {
                for tool in ext.tools().iter() {
                    if seen_names.insert(tool.name.clone()) {
                        catalog.push(ToolCatalogEntry::session_inline(Arc::clone(tool), true));
                    }
                }
            }
        }

        catalog.into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
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
                let async_ops = tool.async_ops_for_output(&output);
                let result = match output {
                    ToolOutput::Json(value) => {
                        let content = match &value {
                            Value::String(s) => s.clone(),
                            _ => serde_json::to_string(&value).unwrap_or_default(),
                        };
                        ToolResult::new(call.id.to_string(), content, false)
                    }
                    ToolOutput::Blocks(blocks) => {
                        ToolResult::with_blocks(call.id.to_string(), blocks, false)
                    }
                };
                return Ok(ToolDispatchOutcome {
                    result,
                    async_ops,
                    session_effects: vec![],
                });
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
                    let async_ops = tool.async_ops_for_output(&output);
                    let result = match output {
                        ToolOutput::Json(value) => {
                            let content = match &value {
                                Value::String(s) => s.clone(),
                                _ => serde_json::to_string(&value).unwrap_or_default(),
                            };
                            ToolResult::new(call.id.to_string(), content, false)
                        }
                        ToolOutput::Blocks(blocks) => {
                            ToolResult::with_blocks(call.id.to_string(), blocks, false)
                        }
                    };
                    return Ok(ToolDispatchOutcome {
                        result,
                        async_ops,
                        session_effects: vec![],
                    });
                }
            }
        }

        if let Some(ref ext) = self.external
            && ext.tools().iter().any(|t| t.name == call.name)
        {
            return ext.dispatch(call).await;
        }

        Err(ToolError::NotFound {
            name: call.name.to_string(),
        })
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        #[allow(unused_mut)]
        let mut update = if let Some(ref ext) = self.external {
            ext.poll_external_updates().await
        } else {
            ExternalToolUpdate::default()
        };

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(ref mgr) = self.job_manager {
            let drained = mgr.drain_completed().await;
            update.background_completions.extend(drained);
        }

        update
    }

    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        self.external
            .as_ref()
            .and_then(|dispatcher| dispatcher.external_tool_surface_snapshot())
    }

    fn capabilities(&self) -> DispatcherCapabilities {
        let mut ops_lifecycle = false;
        #[cfg(not(target_arch = "wasm32"))]
        if self.job_manager.is_some() {
            ops_lifecycle = true;
        }
        if !ops_lifecycle {
            ops_lifecycle = self
                .external
                .as_ref()
                .is_some_and(|ext| ext.capabilities().ops_lifecycle);
        }
        let mut caps = DispatcherCapabilities { ops_lifecycle };
        if let Some(ext) = self.external.as_ref() {
            let ext_caps = ext.capabilities();
            caps.ops_lifecycle |= ext_caps.ops_lifecycle;
        }
        caps
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn OpsLifecycleRegistry>,
        owner_bridge_session_id: SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        let mut owned =
            Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        #[allow(clippy::redundant_clone)]
        // clone needed on non-wasm32 where owner_bridge_session_id is reused
        let rebound_external = match owned.external.take() {
            Some(external)
                if external.capabilities().ops_lifecycle && Arc::strong_count(&external) == 1 =>
            {
                Some(
                    external
                        .bind_ops_lifecycle(Arc::clone(&registry), owner_bridge_session_id.clone())?
                        .into_dispatcher(),
                )
            }
            other => other,
        };

        #[cfg(not(target_arch = "wasm32"))]
        {
            if owned.job_manager.is_none() && rebound_external.is_none() {
                return Err(OpsLifecycleBindError::Unsupported);
            }

            #[cfg_attr(not(feature = "skills"), allow(unused_mut))]
            let mut rebound = CompositeDispatcher::new_with_ops_lifecycle(
                Arc::clone(&owned.task_store),
                &owned.builtin_config,
                owned.project_root.clone(),
                owned.shell_config.clone(),
                rebound_external,
                Some(owner_bridge_session_id.to_string()),
                Some(registry),
                owned.image_tool_results,
            )
            .map_err(|_| OpsLifecycleBindError::Unsupported)?;

            #[cfg(feature = "skills")]
            if let Some(skill_tools) = owned.skill_tools.take() {
                rebound.register_skill_tools(skill_tools);
            }

            Ok(BindOutcome::Bound(Arc::new(rebound)))
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = registry;
            let _ = owner_bridge_session_id;
            let _ = rebound_external;
            Err(OpsLifecycleBindError::Unsupported)
        }
    }

    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn meerkat_core::completion_feed::CompletionEnrichmentProvider>> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(ref mgr) = self.job_manager {
            return Some(Arc::clone(mgr)
                as Arc<
                    dyn meerkat_core::completion_feed::CompletionEnrichmentProvider,
                >);
        }
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::MemoryTaskStore;
    use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
    use meerkat_core::types::SessionId;
    use serde_json::json;
    use tempfile::TempDir;

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
                provenance: None,
            })]);
            Self { tools }
        }
    }

    struct ExactExternalDispatcher {
        catalog: Arc<[meerkat_core::ToolCatalogEntry]>,
    }

    impl ExactExternalDispatcher {
        fn new(entries: &[(&str, bool)]) -> Self {
            let catalog: Vec<meerkat_core::ToolCatalogEntry> = entries
                .iter()
                .map(|(name, currently_callable)| {
                    meerkat_core::ToolCatalogEntry::session_inline(
                        Arc::new(ToolDef {
                            name: (*name).to_string(),
                            description: format!("external tool: {name}"),
                            input_schema: json!({
                                "type": "object",
                                "properties": {},
                                "required": []
                            }),
                            provenance: None,
                        }),
                        *currently_callable,
                    )
                })
                .collect();
            Self {
                catalog: catalog.into(),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ExactExternalDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .filter(|entry| entry.currently_callable)
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
            meerkat_core::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            if self
                .catalog
                .iter()
                .any(|entry| entry.tool.name == call.name && entry.currently_callable)
            {
                return Ok(ToolResult::new(call.id.to_string(), "{}".to_string(), false).into());
            }
            Err(ToolError::not_found(call.name))
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MockExternalDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tools.clone()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            if self.tools.iter().any(|tool| tool.name == call.name) {
                return Ok(ToolResult::new(call.id.to_string(), "{}".to_string(), false).into());
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

    #[test]
    fn exact_catalog_prefers_builtin_winners_over_external_collisions() {
        let store = Arc::new(MemoryTaskStore::new());
        let external: Arc<dyn AgentToolDispatcher> = Arc::new(ExactExternalDispatcher::new(&[
            ("datetime", true),
            ("external_only", true),
        ]));

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

        assert!(
            dispatcher.tool_catalog_capabilities().exact_catalog,
            "composite should be exact when its external dispatcher is exact"
        );

        let catalog = dispatcher.tool_catalog();
        let names: Vec<_> = catalog
            .iter()
            .map(|entry| entry.tool.name.clone())
            .collect();
        assert!(
            names.contains(&"datetime".to_string()),
            "builtin winner should remain in the catalog"
        );
        assert!(
            names.contains(&"external_only".to_string()),
            "non-colliding external winner should remain in the catalog"
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == "datetime")
                .count(),
            1,
            "collision losers must be absent from the exact catalog"
        );
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

        // Builtin JSON outputs should be serialized into text content without
        // losing object structure.
        let call_json = serde_json::value::RawValue::from_string(r"{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "test-str",
            name: "datetime",
            args: &call_json,
        };
        let result = dispatcher
            .dispatch(call)
            .await
            .expect("dispatch should succeed");
        assert!(!result.result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.result.text_content())
            .expect("content should be valid JSON");
        assert!(parsed["iso8601"].is_string());
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
        assert!(!result.result.is_error);
        let parsed: serde_json::Value = serde_json::from_str(&result.result.text_content())
            .expect("content should be valid JSON");
        assert!(
            parsed.get("iso8601").is_some(),
            "should contain iso8601 field"
        );
    }

    #[tokio::test]
    async fn dispatch_forwards_allowed_external_tool_calls() {
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

        let call_json = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "ext-1",
            name: "mob_list",
            args: &call_json,
        };
        let result = dispatcher
            .dispatch(call)
            .await
            .expect("external tool dispatch should succeed");
        assert_eq!(result.result.text_content(), "{}");
    }

    #[tokio::test]
    async fn dispatch_forwards_external_tool_calls_when_allowed_set_contains_name() {
        let store = Arc::new(MemoryTaskStore::new());
        let external: Arc<dyn AgentToolDispatcher> =
            Arc::new(MockExternalDispatcher::new("mob_list", "List active mobs"));
        let mut dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            None,
            None,
            Some(external),
            None,
            true,
        )
        .expect("composite dispatcher should build");
        dispatcher.allowed_tools.insert("mob_list".to_string());

        let call_json = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "ext-2",
            name: "mob_list",
            args: &call_json,
        };
        let result = dispatcher
            .dispatch(call)
            .await
            .expect("external tool dispatch should succeed even with a stale allow-set entry");
        assert_eq!(result.result.text_content(), "{}");
    }

    #[test]
    fn supports_ops_lifecycle_binding_when_shell_tools_present() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(MemoryTaskStore::new());
        let shell_config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let mut config = BuiltinToolConfig::default();
        config.policy.enable.insert("shell".to_string());
        config.policy.enable.insert("shell_job_cancel".to_string());
        let dispatcher = CompositeDispatcher::new(
            store,
            &config,
            None,
            Some(shell_config),
            None,
            Some(SessionId::new().to_string()),
            true,
        )
        .expect("composite dispatcher should build");

        assert!(
            dispatcher.capabilities().ops_lifecycle,
            "shell-enabled composite dispatcher should support ops lifecycle binding"
        );
        assert!(
            !dispatcher
                .shell_job_manager()
                .expect("shell manager")
                .exports_canonical_async_ops(),
            "shell manager should start unbound before canonical registry binding"
        );
    }

    #[tokio::test]
    async fn bind_ops_lifecycle_rebuilds_shell_tools_with_canonical_registry() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(MemoryTaskStore::new());
        let shell_config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let mut config = BuiltinToolConfig::default();
        config.policy.enable.insert("shell".to_string());
        config.policy.enable.insert("shell_job_cancel".to_string());
        let dispatcher = Arc::new(
            CompositeDispatcher::new(
                store,
                &config,
                None,
                Some(shell_config),
                None,
                Some(SessionId::new().to_string()),
                true,
            )
            .expect("composite dispatcher should build"),
        );

        let registry: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new());
        let rebound = dispatcher
            .bind_ops_lifecycle(Arc::clone(&registry), SessionId::new())
            .expect("ops lifecycle binding should succeed")
            .into_dispatcher();

        let call_json = serde_json::value::RawValue::from_string(
            r#"{"command":"sleep 60","background":true}"#.to_string(),
        )
        .unwrap();
        let call = ToolCallView {
            id: "shell-bg",
            name: "shell",
            args: &call_json,
        };
        let outcome = rebound
            .dispatch(call)
            .await
            .expect("background shell dispatch");
        assert_eq!(
            outcome.async_ops.len(),
            1,
            "rebound shell dispatcher must emit canonical async op refs"
        );

        let payload: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).expect("json result");
        let cancel_json = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "job_id": payload["job_id"].as_str().expect("job id"),
            })
            .to_string(),
        )
        .unwrap();
        let cancel = ToolCallView {
            id: "shell-cancel",
            name: "shell_job_cancel",
            args: &cancel_json,
        };
        let _ = rebound
            .dispatch(cancel)
            .await
            .expect("background shell cancel");
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

    #[test]
    #[allow(clippy::panic)]
    fn builtin_tools_have_correct_provenance() {
        use meerkat_core::types::ToolSourceKind;

        let store: Arc<dyn crate::builtin::TaskStore> = Arc::new(MemoryTaskStore::new());

        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap();

        let tools = dispatcher.tools();
        assert!(!tools.is_empty(), "should have at least one builtin tool");
        for tool in tools.iter() {
            let prov = tool
                .provenance
                .as_ref()
                .unwrap_or_else(|| panic!("tool '{}' is missing provenance", tool.name));

            match tool.name.as_str() {
                "shell" | "shell_job_status" | "shell_job_cancel" | "shell_jobs" => {
                    assert_eq!(
                        prov.kind,
                        ToolSourceKind::Shell,
                        "tool '{}' should have Shell provenance",
                        tool.name
                    );
                }
                _ => {
                    assert_eq!(
                        prov.kind,
                        ToolSourceKind::Builtin,
                        "tool '{}' should have Builtin provenance",
                        tool.name
                    );
                }
            }
        }
    }
}
