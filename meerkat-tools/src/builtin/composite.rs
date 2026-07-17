//! Composite dispatcher that combines multiple dispatchers into one.

#[cfg(not(target_arch = "wasm32"))]
use crate::builtin::shell::{JobManager, ShellConfig};
#[cfg(feature = "skills")]
use crate::builtin::skills::SkillToolSet;
use crate::builtin::store::TaskStore;
use crate::builtin::{BuiltinTool, BuiltinToolConfig, BuiltinToolError, ToolOutput};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::BlobStore;
use meerkat_core::ExternalToolUpdate;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::ToolCategoryOverride;
use meerkat_core::ToolDispatchContext;
use meerkat_core::agent::{BindOutcome, DispatcherCapabilities, OpsLifecycleBindError};
use meerkat_core::error::ToolError;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::types::{ContentBlock, SessionId, ToolCallView, ToolDef, ToolResult};
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

/// Convert a `ToolOutput::Json` success into typed tool-result content.
///
/// A bare JSON string is text content and is carried verbatim as a `Text`
/// block; every other JSON shape is preserved as a typed
/// [`ContentBlock::Structured`] payload — structured success is never
/// collapsed into serialized text. A serialization fault is propagated as a
/// typed [`ToolError`] rather than being laundered into silently-empty
/// successful output.
fn json_output_blocks(name: &str, value: &Value) -> Result<Vec<ContentBlock>, ToolError> {
    match value {
        Value::String(s) => Ok(ContentBlock::text_vec(s.clone())),
        _ => ContentBlock::structured(value)
            .map(|block| vec![block])
            .map_err(|err| ToolError::ExecutionFailed {
                message: format!("failed to serialize JSON tool output for '{name}': {err}"),
            }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct ImageGenerationToolBinding {
    runtime: crate::builtin::image_generation::ImageGenerationToolRuntime,
    visibility: ToolCategoryOverride,
}

#[cfg(not(target_arch = "wasm32"))]
struct WebSearchToolBinding {
    executor: Arc<dyn meerkat_llm_core::WebSearchExecutor>,
    visibility: ToolCategoryOverride,
}

#[cfg(not(target_arch = "wasm32"))]
struct BlobToolBinding {
    blob_store: Arc<dyn BlobStore>,
}

#[cfg(not(target_arch = "wasm32"))]
const IMAGE_GENERATION_TOOL_NAMES: &[&str] = &["generate_image"];
#[cfg(not(target_arch = "wasm32"))]
const WEB_SEARCH_TOOL_NAMES: &[&str] = &["web_search"];
#[cfg(not(target_arch = "wasm32"))]
const BLOB_FILE_TOOL_NAMES: &[&str] = &["blob_save_file", "blob_load_file", "blob_inspect"];

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
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(dead_code)]
    job_manager: Option<Arc<JobManager>>,
    #[cfg(not(target_arch = "wasm32"))]
    image_generation_runtime: Option<ImageGenerationToolBinding>,
    #[cfg(not(target_arch = "wasm32"))]
    web_search_runtime: Option<WebSearchToolBinding>,
    #[cfg(not(target_arch = "wasm32"))]
    blob_tools: Option<BlobToolBinding>,
    allowed_tools: HashSet<String>,
}

impl CompositeDispatcher {
    /// Remove the concrete projection of one optional builtin family before a
    /// new registration becomes its canonical owner. Registrations are
    /// replace-in-place updates, not append-only link-order candidates.
    #[cfg(not(target_arch = "wasm32"))]
    fn remove_optional_builtin_family(&mut self, names: &[&str]) {
        self.builtin_tools
            .retain(|tool| !names.contains(&tool.name()));
        for name in names {
            self.allowed_tools.remove(*name);
        }
    }

    /// Create a new composite dispatcher with builtin tools.
    ///
    /// view_image is always registered; visibility for non-image models is
    /// controlled at the factory level via `ToolScope` external filters.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(
        task_store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        project_root: Option<PathBuf>,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
    ) -> Result<Self, CompositeDispatcherError> {
        Self::new_with_ops_lifecycle(
            task_store,
            config,
            project_root,
            shell_config,
            external,
            session_id,
            None,
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
    ) -> Result<Self, CompositeDispatcherError> {
        let mut builtin_tools: Vec<Arc<dyn BuiltinTool>> = Vec::new();
        let shell_session_id = session_id.clone();
        // The project root is a concrete authority threaded by the caller (the
        // factory seeds it from the session store parent / shell config). We must
        // never fall back to the ambient process CWD: that silently re-derives a
        // filesystem root the caller is responsible for owning, leaking
        // whichever directory the host process happens to be in into
        // apply_patch / view_image. Fail closed when no concrete root is supplied
        // (dogma row #299).
        let project_root = project_root
            .or_else(|| shell_config.as_ref().map(|cfg| cfg.project_root.clone()))
            .ok_or_else(|| CompositeDispatcherError::ToolInitFailed {
                name: "apply_patch".into(),
                message: "failed to resolve project root: no project_root or shell_config supplied"
                    .to_string(),
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
                    manager = manager.with_owner_bridge_session_id(session_id);
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

        // NOTE: view_image is always kept in allowed_tools. Visibility gating
        // for non-image models is handled at the factory level via ToolScope
        // external filters, enabling hot-swap to reveal view_image later.

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
            job_manager,
            image_generation_runtime: None,
            web_search_runtime: None,
            blob_tools: None,
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
            #[cfg(not(target_arch = "wasm32"))]
            job_manager: None,
            #[cfg(not(target_arch = "wasm32"))]
            image_generation_runtime: None,
            #[cfg(not(target_arch = "wasm32"))]
            web_search_runtime: None,
            #[cfg(not(target_arch = "wasm32"))]
            blob_tools: None,
            allowed_tools,
        })
    }

    /// Register skill discovery tools (browse_skills, load_skill).
    #[cfg(feature = "skills")]
    pub fn register_skill_tools(&mut self, tool_set: SkillToolSet) {
        let resolved_policy = self.builtin_config.resolve();
        for tool in tool_set.tools() {
            let name = tool.name();
            if resolved_policy.is_enabled(name, tool.default_enabled()) {
                self.allowed_tools.insert(name.to_string());
            }
        }
        self.skill_tools = Some(tool_set);
    }

    /// Register the session-owned assistant image generation builtin.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn register_image_generation_tool(
        &mut self,
        runtime: crate::builtin::image_generation::ImageGenerationToolRuntime,
        visibility: ToolCategoryOverride,
    ) {
        self.remove_optional_builtin_family(IMAGE_GENERATION_TOOL_NAMES);
        self.image_generation_runtime = Some(ImageGenerationToolBinding {
            runtime: runtime.clone(),
            visibility,
        });
        // Inherit means visible when the session-owned image substrate is wired.
        if !visibility.resolve(true) {
            return;
        }
        let tool = Arc::new(crate::builtin::image_generation::GenerateImageTool::new(
            runtime,
        ));
        self.allowed_tools.insert(tool.name().to_string());
        self.builtin_tools.push(tool);
    }

    /// Register the optional Meerkat-owned web-search fallback builtin.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn register_web_search_tool(
        &mut self,
        executor: Arc<dyn meerkat_llm_core::WebSearchExecutor>,
        visibility: ToolCategoryOverride,
    ) {
        self.remove_optional_builtin_family(WEB_SEARCH_TOOL_NAMES);
        self.web_search_runtime = Some(WebSearchToolBinding {
            executor: Arc::clone(&executor),
            visibility,
        });
        // Inherit is intentionally off. Models with native web search use
        // provider-native tools; callers explicitly enable this fallback for
        // models such as realtime live models.
        if !visibility.resolve(false) {
            return;
        }
        let tool = Arc::new(crate::builtin::web_search::WebSearchTool::new(Arc::clone(
            &executor,
        )));
        self.allowed_tools.insert(tool.name().to_string());
        self.builtin_tools.push(tool);
    }

    /// Register blob file bridge builtins backed by the session blob store.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn register_blob_file_tools(&mut self, blob_store: Arc<dyn BlobStore>) {
        self.remove_optional_builtin_family(BLOB_FILE_TOOL_NAMES);
        self.blob_tools = Some(BlobToolBinding {
            blob_store: Arc::clone(&blob_store),
        });
        let Some(project_root) = self.project_root.clone() else {
            return;
        };
        use crate::builtin::utility::{BlobInspectTool, BlobLoadFileTool, BlobSaveFileTool};
        let tools: [Arc<dyn BuiltinTool>; 3] = [
            Arc::new(BlobSaveFileTool::new(
                project_root.clone(),
                Arc::clone(&blob_store),
            )),
            Arc::new(BlobLoadFileTool::new(project_root, Arc::clone(&blob_store))),
            Arc::new(BlobInspectTool::new(Arc::clone(&blob_store))),
        ];
        let resolved_policy = self.builtin_config.resolve();
        for tool in tools {
            let name = tool.name().to_string();
            if resolved_policy.is_enabled(&name, tool.default_enabled()) {
                self.allowed_tools.insert(name);
            }
            self.builtin_tools.push(tool);
        }
    }

    /// Get usage instructions for all enabled tools.
    pub fn usage_instructions(&self) -> String {
        let mut out = String::from("# Available Tools\n\n");
        for tool in &self.builtin_tools {
            if matches!(
                self.resolve_tool_owner(tool.name()),
                ResolvedToolOwner::Builtin(_)
            ) {
                use std::fmt::Write;
                let _ = write!(out, "## {}\n{}\n\n", tool.name(), tool.def().description);
            }
        }
        if let Some(ref ext) = self.external {
            let mut wrote_external_header = false;
            for tool in ext.tools().iter() {
                if !matches!(
                    self.resolve_tool_owner(&tool.name),
                    ResolvedToolOwner::External
                ) {
                    continue;
                }
                if !wrote_external_header {
                    out.push_str(
                        "## External tools\nProvided by integrated runtimes/services.\n\n",
                    );
                    wrote_external_header = true;
                }
                {
                    use std::fmt::Write;
                    let _ = write!(out, "## {}\n{}\n\n", tool.name, tool.description);
                }
            }
        }
        out
    }

    /// Single owner of the tool-name precedence decision.
    ///
    /// Every surface that answers "who owns tool name X?" — advertisement
    /// ([`AgentToolDispatcher::tools`], [`AgentToolDispatcher::tool_catalog`],
    /// [`Self::usage_instructions`]) and execution
    /// ([`AgentToolDispatcher::dispatch_with_context`]) — derives the winner
    /// from this one function, so the advertised contract and the dispatch
    /// behavior can never disagree.
    ///
    /// Precedence: policy-allowed builtin > policy-allowed skill tool >
    /// external. A builtin/skill tool that exists but is policy-disabled does
    /// NOT shadow a colliding external tool: the external tool both advertises
    /// and dispatches. Only when no other source claims the name does the
    /// disabled local tool resolve to [`ResolvedToolOwner::PolicyDenied`]
    /// (a 403-shaped fault, distinct from 404).
    fn resolve_tool_owner(&self, name: &str) -> ResolvedToolOwner<'_> {
        let builtin = self
            .builtin_tools
            .iter()
            .find(|tool| tool.name() == name)
            .map(Arc::as_ref);
        if let Some(tool) = builtin
            && self.allowed_tools.contains(name)
        {
            return ResolvedToolOwner::Builtin(tool);
        }
        #[cfg(feature = "skills")]
        let skill = self
            .skill_tools
            .as_ref()
            .and_then(|set| set.tools().into_iter().find(|tool| tool.name() == name));
        #[cfg(feature = "skills")]
        if let Some(tool) = skill
            && self.allowed_tools.contains(name)
        {
            return ResolvedToolOwner::Skill(tool);
        }
        if let Some(ref ext) = self.external {
            let advertised = if ext.tool_catalog_capabilities().exact_catalog {
                ext.tool_catalog()
                    .iter()
                    .any(|entry| entry.tool.name == name)
            } else {
                ext.tools().iter().any(|tool| tool.name == name)
            };
            if advertised {
                return ResolvedToolOwner::External;
            }
        }
        let local_exists = builtin.is_some();
        #[cfg(feature = "skills")]
        let local_exists = local_exists || skill.is_some();
        if local_exists {
            ResolvedToolOwner::PolicyDenied
        } else {
            ResolvedToolOwner::NotFound
        }
    }

    /// Execute a locally-owned (builtin or skill) tool and convert its output
    /// into a [`ToolDispatchOutcome`]. Shared by the builtin and skill dispatch
    /// arms so the output-conversion policy has exactly one implementation.
    async fn call_local_tool(
        &self,
        tool: &dyn BuiltinTool,
        call: ToolCallView<'_>,
        args: Value,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let output = tool.call(args).await.map_err(|e| match e {
            BuiltinToolError::InvalidArgs(msg) => ToolError::InvalidArguments {
                name: call.name.into(),
                reason: msg,
            },
            BuiltinToolError::ExecutionFailed(msg) => ToolError::ExecutionFailed { message: msg },
            BuiltinToolError::TaskError(te) => ToolError::ExecutionFailed { message: te },
        })?;
        let async_ops = tool.async_ops_for_output(&output);
        match output {
            ToolOutput::Json(value) => {
                let content = json_output_blocks(call.name, &value)?;
                Ok(ToolDispatchOutcome::new(
                    ToolResult::with_blocks(call.id.to_string(), content, false),
                    async_ops,
                    vec![],
                ))
            }
            ToolOutput::JsonWithEffects {
                value,
                session_effects,
            } => {
                let content = json_output_blocks(call.name, &value)?;
                Ok(ToolDispatchOutcome::new(
                    ToolResult::with_blocks(call.id.to_string(), content, false),
                    async_ops,
                    session_effects,
                ))
            }
            ToolOutput::Blocks(blocks) => Ok(ToolDispatchOutcome::new(
                ToolResult::with_blocks(call.id.to_string(), blocks, false),
                async_ops,
                vec![],
            )),
        }
    }

    /// Return the shared shell job manager when shell tools are enabled.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn shell_job_manager(&self) -> Option<Arc<JobManager>> {
        self.job_manager.clone()
    }
}

/// The winner of a tool-name precedence decision, resolved exclusively by
/// [`CompositeDispatcher::resolve_tool_owner`]. Advertisement and dispatch
/// both consume this single derivation; neither re-derives precedence locally.
enum ResolvedToolOwner<'a> {
    /// A policy-allowed builtin tool owns the name.
    Builtin(&'a dyn BuiltinTool),
    /// A policy-allowed skill tool owns the name.
    #[cfg(feature = "skills")]
    Skill(&'a dyn BuiltinTool),
    /// The external dispatcher advertises and owns the name.
    External,
    /// A local (builtin/skill) tool exists under this name but is
    /// policy-disabled, and no other source claims the name: the call is
    /// denied, not missing.
    PolicyDenied,
    /// No source claims the name.
    NotFound,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for CompositeDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut tools = Vec::new();

        // Each source advertises exactly the names it wins per the single
        // precedence owner (`resolve_tool_owner`), so advertisement can never
        // disagree with dispatch about who handles a colliding name.
        for tool in &self.builtin_tools {
            if matches!(
                self.resolve_tool_owner(tool.name()),
                ResolvedToolOwner::Builtin(_)
            ) {
                tools.push(Arc::new(tool.def()));
            }
        }

        #[cfg(feature = "skills")]
        if let Some(ref skill) = self.skill_tools {
            for tool in skill.tools() {
                if matches!(
                    self.resolve_tool_owner(tool.name()),
                    ResolvedToolOwner::Skill(_)
                ) {
                    tools.push(Arc::new(tool.def()));
                }
            }
        }

        if let Some(ref ext) = self.external {
            // Intra-source dedup only; precedence against local tools is
            // owned by `resolve_tool_owner`.
            let mut seen_external = HashSet::new();
            for tool in ext.tools().iter() {
                if seen_external.insert(tool.name.to_string())
                    && matches!(
                        self.resolve_tool_owner(&tool.name),
                        ResolvedToolOwner::External
                    )
                {
                    tools.push(Arc::clone(tool));
                }
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

        // Each source contributes exactly the names it wins per the single
        // precedence owner (`resolve_tool_owner`).
        for tool in &self.builtin_tools {
            if matches!(
                self.resolve_tool_owner(tool.name()),
                ResolvedToolOwner::Builtin(_)
            ) {
                catalog.push(ToolCatalogEntry::session_inline(Arc::new(tool.def()), true));
            }
        }

        #[cfg(feature = "skills")]
        if let Some(ref skill) = self.skill_tools {
            for tool in skill.tools() {
                if matches!(
                    self.resolve_tool_owner(tool.name()),
                    ResolvedToolOwner::Skill(_)
                ) {
                    catalog.push(ToolCatalogEntry::session_inline(Arc::new(tool.def()), true));
                }
            }
        }

        if let Some(ref ext) = self.external {
            // Intra-source dedup only; precedence against local tools is
            // owned by `resolve_tool_owner`.
            let mut seen_external = HashSet::new();
            if ext.tool_catalog_capabilities().exact_catalog {
                for entry in ext.tool_catalog().iter() {
                    if seen_external.insert(entry.tool.name.to_string())
                        && matches!(
                            self.resolve_tool_owner(&entry.tool.name),
                            ResolvedToolOwner::External
                        )
                    {
                        catalog.push(entry.clone());
                    }
                }
            } else {
                for tool in ext.tools().iter() {
                    if seen_external.insert(tool.name.to_string())
                        && matches!(
                            self.resolve_tool_owner(&tool.name),
                            ResolvedToolOwner::External
                        )
                    {
                        catalog.push(ToolCatalogEntry::session_inline(Arc::clone(tool), true));
                    }
                }
            }
        }

        catalog.into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context(call, &ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let args: Value =
            serde_json::from_str(call.args.get()).map_err(|e| ToolError::InvalidArguments {
                name: call.name.into(),
                reason: e.to_string(),
            })?;
        // Dispatch consumes the same winner derivation advertisement uses
        // (`resolve_tool_owner`): an advertised tool always dispatches to the
        // advertised owner, and a policy-disabled local tool never shadows a
        // colliding external tool. Wave B (V7): a local tool that exists but
        // is policy-disabled — with no other owner for the name — is denied,
        // not missing, so surfaces can distinguish 403 from 404.
        match self.resolve_tool_owner(call.name) {
            ResolvedToolOwner::Builtin(tool) => self.call_local_tool(tool, call, args).await,
            #[cfg(feature = "skills")]
            ResolvedToolOwner::Skill(tool) => self.call_local_tool(tool, call, args).await,
            ResolvedToolOwner::External => {
                let Some(ext) = self.external.as_ref() else {
                    // `External` is only resolved when an external dispatcher
                    // exists; fail closed rather than fabricate a result.
                    return Err(ToolError::NotFound {
                        name: call.name.into(),
                    });
                };
                if ext.tool_catalog_capabilities().exact_catalog {
                    let catalog = ext.tool_catalog();
                    if let Some(entry) = catalog.iter().find(|entry| entry.tool.name == call.name)
                        && let Some(reason) = entry.callability.unavailable_reason()
                    {
                        return Err(ToolError::unavailable(call.name, reason));
                    }
                }
                ext.dispatch_with_context(call, context).await
            }
            ResolvedToolOwner::PolicyDenied => Err(ToolError::AccessDenied {
                name: call.name.into(),
            }),
            ResolvedToolOwner::NotFound => Err(ToolError::NotFound {
                name: call.name.into(),
            }),
        }
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
            // Cleanup marker only. Agent-visible completion publication comes
            // from the canonical completion feed, not external update polling.
            let _ = mgr.drain_completed().await;
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
            if owned.job_manager.is_none()
                && rebound_external.is_none()
                && owned.image_generation_runtime.is_none()
                && owned.web_search_runtime.is_none()
                && owned.blob_tools.is_none()
            {
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
            )
            .map_err(|_| OpsLifecycleBindError::Unsupported)?;

            #[cfg(feature = "skills")]
            if let Some(skill_tools) = owned.skill_tools.take() {
                rebound.register_skill_tools(skill_tools);
            }
            if let Some(binding) = owned.image_generation_runtime.take() {
                rebound.register_image_generation_tool(binding.runtime, binding.visibility);
            }
            if let Some(binding) = owned.web_search_runtime.take() {
                rebound.register_web_search_tool(binding.executor, binding.visibility);
            }
            if let Some(binding) = owned.blob_tools.take() {
                rebound.register_blob_file_tools(binding.blob_store);
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

    fn bind_mcp_server_lifecycle_handle(
        &self,
        handle: Arc<dyn meerkat_core::handles::McpServerLifecycleHandle>,
    ) {
        // The external child is the only nested dispatcher that owns MCP
        // handshake lifecycle. Without this forwarding, the factory's
        // session-time bind stops at the composite and the nested MCP
        // adapter stays attached to its construction-time authority.
        if let Some(ext) = self.external.as_ref() {
            ext.bind_mcp_server_lifecycle_handle(handle);
        }
    }

    fn bind_external_tool_surface_handle(
        &self,
        handle: Arc<dyn meerkat_core::handles::ExternalToolSurfaceHandle>,
    ) {
        // Same forwarding rule as the MCP lifecycle bind: dynamic external
        // tool surfaces live on the nested external dispatcher.
        if let Some(ext) = self.external.as_ref() {
            ext.bind_external_tool_surface_handle(handle);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::builtin::MemoryTaskStore;
    use crate::builtin::ToolPolicyLayer;
    use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
    use meerkat_core::types::SessionId;
    use meerkat_core::{BlobId, BlobPayload, BlobRef, BlobStoreError};
    use serde_json::json;
    use std::collections::HashMap;
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    /// Concrete project root for tests that do not exercise the project-root
    /// dependent tools (apply_patch / view_image / shell). The composite now
    /// fails closed without a concrete root rather than falling back to the
    /// ambient process CWD (dogma row #299), so tests must supply one explicitly.
    /// The crate manifest dir is a stable, real directory and is never the
    /// laundered ambient CWD.
    fn test_project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[derive(Default)]
    struct TestBlobStore {
        blobs: Mutex<HashMap<BlobId, BlobPayload>>,
    }

    #[async_trait]
    impl BlobStore for TestBlobStore {
        async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
            let blob_id = BlobId::new(format!("sha256:{media_type}:{data}"));
            self.blobs.lock().await.insert(
                blob_id.clone(),
                BlobPayload {
                    blob_id: blob_id.clone(),
                    media_type: media_type.to_string(),
                    data: data.to_string(),
                },
            );
            Ok(BlobRef {
                blob_id,
                media_type: media_type.to_string(),
            })
        }

        async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
            self.blobs
                .lock()
                .await
                .get(blob_id)
                .cloned()
                .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))
        }

        async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
            self.blobs.lock().await.remove(blob_id);
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            false
        }
    }

    struct RejectingImageMachine {
        marker: &'static str,
    }

    #[async_trait]
    impl crate::builtin::image_generation::ImageGenerationMachine for RejectingImageMachine {
        async fn session_model_routing_status(
            &self,
            _session_id: &SessionId,
        ) -> Result<
            meerkat_core::image_generation::SessionModelRoutingStatus,
            meerkat_runtime::RuntimeDriverError,
        > {
            Err(meerkat_runtime::RuntimeDriverError::ValidationFailed {
                reason: self.marker.to_string(),
            })
        }

        async fn begin_image_operation(
            &self,
            _session_id: &SessionId,
            _request: meerkat_runtime::ImageOperationRoutingRequest,
        ) -> Result<meerkat_runtime::ImageOperationRoutingResult, meerkat_runtime::RuntimeDriverError>
        {
            unreachable!("routing status rejection must precede begin")
        }

        async fn deny_image_operation_plan(
            &self,
            _session_id: &SessionId,
            _operation_id: meerkat_core::ImageOperationId,
            _reason: meerkat_core::image_generation::ImageOperationDenialReason,
        ) -> Result<
            meerkat_core::image_generation::ImageOperationPhase,
            meerkat_runtime::RuntimeDriverError,
        > {
            unreachable!("routing status rejection must precede denial")
        }

        async fn activate_image_operation_override(
            &self,
            _session_id: &SessionId,
            _operation_id: meerkat_core::ImageOperationId,
        ) -> Result<
            meerkat_core::image_generation::ImageOperationPhase,
            meerkat_runtime::RuntimeDriverError,
        > {
            unreachable!("routing status rejection must precede activation")
        }

        async fn classify_image_operation_terminal(
            &self,
            _session_id: &SessionId,
            _operation_id: meerkat_core::ImageOperationId,
            _observation: meerkat_core::image_generation::ImageProviderTerminalObservation,
            _provider_text: meerkat_core::image_generation::ProviderTextDisposition,
        ) -> Result<
            meerkat_core::image_generation::ImageOperationTerminalClass,
            meerkat_runtime::RuntimeDriverError,
        > {
            unreachable!("routing status rejection must precede classification")
        }

        async fn complete_image_operation(
            &self,
            _session_id: &SessionId,
            _operation_id: meerkat_core::ImageOperationId,
            _terminal: meerkat_core::image_generation::ImageOperationTerminalClass,
        ) -> Result<
            meerkat_core::image_generation::ImageOperationPhase,
            meerkat_runtime::RuntimeDriverError,
        > {
            unreachable!("routing status rejection must precede completion")
        }

        async fn restore_image_operation_override(
            &self,
            _session_id: &SessionId,
            _operation_id: meerkat_core::ImageOperationId,
        ) -> Result<
            meerkat_core::image_generation::ImageOperationPhase,
            meerkat_runtime::RuntimeDriverError,
        > {
            unreachable!("routing status rejection must precede restore")
        }
    }

    struct NeverImagePlanner;

    impl meerkat_core::image_generation::ImageGenerationPlanner for NeverImagePlanner {
        fn resolve_image_generation_plan(
            &self,
            _status: &meerkat_core::image_generation::SessionModelRoutingStatus,
            _operation_id: meerkat_core::ImageOperationId,
            _request: &meerkat_core::image_generation::GenerateImageRequest,
        ) -> Result<
            meerkat_core::image_generation::ImageGenerationResolvedPlan,
            meerkat_core::image_generation::ImageOperationDenialReason,
        > {
            unreachable!("routing status rejection must precede planning")
        }

        fn infer_provider_for_model(
            &self,
            _model: &str,
        ) -> Option<meerkat_core::image_generation::ProviderId> {
            None
        }
    }

    struct NeverImageExecutor;

    #[async_trait]
    impl meerkat_llm_core::ImageGenerationExecutor for NeverImageExecutor {
        async fn execute_image_generation(
            &self,
            _request: meerkat_llm_core::ProviderImageGenerationRequest,
        ) -> Result<meerkat_llm_core::ProviderImageGenerationOutput, meerkat_llm_core::LlmError>
        {
            unreachable!("routing status rejection must precede execution")
        }
    }

    fn rejecting_image_runtime(
        marker: &'static str,
    ) -> crate::builtin::image_generation::ImageGenerationToolRuntime {
        crate::builtin::image_generation::ImageGenerationToolRuntime {
            session_id: SessionId::new(),
            machine: Arc::new(RejectingImageMachine { marker }),
            planner: Arc::new(NeverImagePlanner),
            blob_store: Arc::new(TestBlobStore::default()),
            executor: Arc::new(NeverImageExecutor),
        }
    }

    struct MarkerWebSearchExecutor {
        marker: &'static str,
    }

    #[async_trait]
    impl meerkat_llm_core::WebSearchExecutor for MarkerWebSearchExecutor {
        async fn execute_web_search(
            &self,
            request: meerkat_core::web_search::WebSearchRequest,
        ) -> Result<meerkat_core::web_search::WebSearchResult, meerkat_llm_core::LlmError> {
            Ok(meerkat_core::web_search::WebSearchResult {
                status: meerkat_core::WebSearchStatus::Completed,
                query: request.query,
                provider: None,
                model: None,
                answer: Some(self.marker.to_string()),
                evidence: Vec::new(),
                native_events: Vec::new(),
                error: None,
                checked_at: chrono::Utc::now(),
            })
        }
    }

    struct MockExternalDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    impl MockExternalDispatcher {
        fn new(name: &str, description: &str) -> Self {
            let tools: Arc<[Arc<ToolDef>]> = Arc::from([Arc::new(ToolDef {
                name: name.into(),
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

    struct ContextAwareExactExternalDispatcher {
        catalog: Arc<[meerkat_core::ToolCatalogEntry]>,
    }

    impl ExactExternalDispatcher {
        fn new(entries: &[(&str, bool)]) -> Self {
            let catalog: Vec<meerkat_core::ToolCatalogEntry> = entries
                .iter()
                .map(|(name, currently_callable)| {
                    meerkat_core::ToolCatalogEntry::session_inline(
                        Arc::new(ToolDef {
                            name: (*name).into(),
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

    impl ContextAwareExactExternalDispatcher {
        fn new() -> Self {
            Self {
                catalog: Arc::from([meerkat_core::ToolCatalogEntry::session_inline(
                    Arc::new(ToolDef {
                        name: "inspect_context".into(),
                        description: "inspect context".to_string(),
                        input_schema: json!({
                            "type": "object",
                            "properties": {},
                            "required": []
                        }),
                        provenance: None,
                    }),
                    true,
                )]),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ExactExternalDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .filter(|entry| entry.currently_callable())
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
                .any(|entry| entry.tool.name == call.name && entry.currently_callable())
            {
                return Ok(ToolResult::new(call.id.to_string(), "{}".to_string(), false).into());
            }
            Err(ToolError::not_found(call.name))
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ContextAwareExactExternalDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .filter(|entry| entry.currently_callable())
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
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"saw_context_image": false}).to_string(),
                false,
            )
            .into())
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &ToolDispatchContext,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            let saw_context_image = context
                .current_turn()
                .and_then(|turn| turn.image_ref(0))
                .and_then(|image_ref| context.current_turn_image(image_ref))
                .is_some();
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"saw_context_image": saw_context_image}).to_string(),
                false,
            )
            .into())
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
    fn composite_fails_closed_without_concrete_project_root() {
        // With neither a concrete `project_root` nor a `shell_config`, the
        // composite must fail closed with a typed `ToolInitFailed` rather than
        // laundering the ambient process CWD into apply_patch / view_image
        // (dogma row #299).
        let store = Arc::new(MemoryTaskStore::new());
        let err =
            CompositeDispatcher::new(store, &BuiltinToolConfig::default(), None, None, None, None)
                .err()
                .expect("composite must fail closed without a concrete project root");
        match err {
            CompositeDispatcherError::ToolInitFailed { name, message } => {
                assert_eq!(name, "apply_patch");
                assert!(
                    message.contains("project root"),
                    "error must explain the missing project root: {message}"
                );
            }
            other => panic!("expected ToolInitFailed, got {other:?}"),
        }
    }

    #[test]
    fn composite_resolves_project_root_from_shell_config_when_root_unset() {
        // The shell config's project_root is a concrete authority and must be
        // honored when no explicit project_root is threaded — without falling
        // back to the ambient CWD.
        let store = Arc::new(MemoryTaskStore::new());
        let temp_dir = TempDir::new().expect("temp dir");
        let shell_config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            None,
            Some(shell_config),
            None,
            None,
        )
        .expect("composite should resolve project root from shell config");
        assert!(
            dispatcher
                .tools()
                .iter()
                .any(|tool| tool.name == "apply_patch"),
            "apply_patch must register once a concrete project root is resolved"
        );
    }

    struct BindRecordingExternalDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        mcp_handles_bound: Arc<std::sync::atomic::AtomicUsize>,
        surface_handles_bound: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl BindRecordingExternalDispatcher {
        fn new(
            mcp_handles_bound: Arc<std::sync::atomic::AtomicUsize>,
            surface_handles_bound: Arc<std::sync::atomic::AtomicUsize>,
        ) -> Self {
            Self {
                tools: Arc::from([]),
                mcp_handles_bound,
                surface_handles_bound,
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl AgentToolDispatcher for BindRecordingExternalDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tools.clone()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::not_found(call.name))
        }

        fn bind_mcp_server_lifecycle_handle(
            &self,
            _handle: Arc<dyn meerkat_core::handles::McpServerLifecycleHandle>,
        ) {
            self.mcp_handles_bound
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        fn bind_external_tool_surface_handle(
            &self,
            _handle: Arc<dyn meerkat_core::handles::ExternalToolSurfaceHandle>,
        ) {
            self.surface_handles_bound
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// M2 regression: the factory's session-time handle binds must reach the
    /// nested external dispatcher when builtins compose it. Before this, the
    /// composite swallowed both binds (only `bind_ops_lifecycle` forwarded),
    /// so a nested MCP adapter stayed attached to its construction-time
    /// authority and session-owned MCP wiring never worked with builtins.
    #[test]
    fn composite_forwards_handle_binds_to_external_dispatcher() {
        let mcp_handles_bound = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let surface_handles_bound = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let external: Arc<dyn AgentToolDispatcher> =
            Arc::new(BindRecordingExternalDispatcher::new(
                Arc::clone(&mcp_handles_bound),
                Arc::clone(&surface_handles_bound),
            ));
        let store = Arc::new(MemoryTaskStore::new());
        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            Some(test_project_root()),
            None,
            Some(external),
            None,
        )
        .expect("composite dispatcher should build");

        dispatcher.bind_mcp_server_lifecycle_handle(Arc::new(
            meerkat_runtime::handles::RuntimeMcpServerLifecycleHandle::ephemeral(),
        ));
        dispatcher.bind_external_tool_surface_handle(Arc::new(
            meerkat_runtime::handles::RuntimeExternalToolSurfaceHandle::ephemeral(),
        ));

        assert_eq!(
            mcp_handles_bound.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "bind_mcp_server_lifecycle_handle must forward to the external dispatcher"
        );
        assert_eq!(
            surface_handles_bound.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "bind_external_tool_surface_handle must forward to the external dispatcher"
        );
    }

    #[test]
    fn usage_instructions_include_external_tools() {
        let store = Arc::new(MemoryTaskStore::new());
        let external: Arc<dyn AgentToolDispatcher> =
            Arc::new(MockExternalDispatcher::new("mob_list", "List active mobs"));

        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            Some(test_project_root()),
            None,
            Some(external),
            None,
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
            Some(test_project_root()),
            None,
            Some(external),
            None,
        )
        .expect("composite dispatcher should build");

        assert!(
            dispatcher.tool_catalog_capabilities().exact_catalog,
            "composite should be exact when its external dispatcher is exact"
        );

        let catalog = dispatcher.tool_catalog();
        let names: Vec<_> = catalog
            .iter()
            .map(|entry| entry.tool.name.to_string())
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
    async fn exact_external_non_callable_winner_dispatches_unavailable() {
        let store = Arc::new(MemoryTaskStore::new());
        let external: Arc<dyn AgentToolDispatcher> =
            Arc::new(ExactExternalDispatcher::new(&[("external_only", false)]));

        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            Some(test_project_root()),
            None,
            Some(external),
            None,
        )
        .expect("composite dispatcher should build");

        let catalog = dispatcher.tool_catalog();
        assert!(
            !catalog
                .iter()
                .find(|entry| entry.tool.name == "external_only")
                .expect("external catalog entry")
                .currently_callable()
        );

        let call_json = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "ext-unavailable",
            name: "external_only",
            args: &call_json,
        };
        let result = dispatcher.dispatch(call).await;
        assert!(matches!(result, Err(ToolError::Unavailable { .. })));
    }

    #[tokio::test]
    async fn disabled_builtin_does_not_shadow_colliding_external_tool() {
        // The single precedence owner (`resolve_tool_owner`) must make
        // advertisement and dispatch agree: when a builtin is policy-disabled
        // and an external tool collides on its name, the external tool is the
        // winner on BOTH surfaces — it is advertised AND it dispatches.
        // (Previously dispatch returned AccessDenied for a name the catalog
        // advertised as an external tool.)
        let store = Arc::new(MemoryTaskStore::new());
        let config = BuiltinToolConfig {
            policy: ToolPolicyLayer::new().disable_tool("datetime"),
            ..Default::default()
        };
        let external: Arc<dyn AgentToolDispatcher> =
            Arc::new(ExactExternalDispatcher::new(&[("datetime", true)]));

        let dispatcher = CompositeDispatcher::new(
            store,
            &config,
            Some(test_project_root()),
            None,
            Some(external),
            None,
        )
        .expect("composite dispatcher should build");

        // Advertised exactly once, as the external entry.
        let catalog = dispatcher.tool_catalog();
        let datetime_entries: Vec<_> = catalog
            .iter()
            .filter(|entry| entry.tool.name == "datetime")
            .collect();
        assert_eq!(datetime_entries.len(), 1, "exactly one winner per name");
        assert_eq!(
            datetime_entries[0].tool.description, "external tool: datetime",
            "the external tool must be the advertised winner"
        );

        // Dispatch agrees with advertisement: it routes to the external tool.
        let call_json = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "collide-1",
            name: "datetime",
            args: &call_json,
        };
        let outcome = dispatcher
            .dispatch(call)
            .await
            .expect("advertised external winner must dispatch, not AccessDenied");
        assert_eq!(outcome.result.text_content(), "{}");
    }

    #[tokio::test]
    async fn disabled_builtin_without_collision_is_policy_denied_not_missing() {
        // Wave B (V7) semantics preserved: a policy-disabled builtin with no
        // other owner for the name is AccessDenied (403), not NotFound (404).
        let store = Arc::new(MemoryTaskStore::new());
        let config = BuiltinToolConfig {
            policy: ToolPolicyLayer::new().disable_tool("datetime"),
            ..Default::default()
        };
        let dispatcher =
            CompositeDispatcher::new(store, &config, Some(test_project_root()), None, None, None)
                .expect("composite dispatcher should build");

        assert!(
            !dispatcher
                .tool_catalog()
                .iter()
                .any(|entry| entry.tool.name == "datetime"),
            "a policy-denied builtin must not be advertised"
        );

        let call_json = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "denied-1",
            name: "datetime",
            args: &call_json,
        };
        let err = dispatcher.dispatch(call).await.unwrap_err();
        assert!(matches!(err, ToolError::AccessDenied { .. }));
    }

    #[tokio::test]
    async fn dispatch_json_string_produces_text_result() {
        let store = Arc::new(MemoryTaskStore::new());
        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            Some(test_project_root()),
            None,
            None,
            None,
        )
        .expect("composite dispatcher should build");

        // Builtin JSON outputs arrive as typed Structured blocks; the text
        // projection of that block is the raw JSON, so text-oriented
        // consumers keep a lossless view without the dispatcher collapsing
        // the structured payload itself.
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

    #[test]
    fn json_output_blocks_keeps_strings_text_and_objects_structured() {
        // Bare strings carry verbatim as text content (no JSON quoting).
        let s = json_output_blocks("t", &Value::String("hello".into()))
            .expect("string content should render");
        assert_eq!(
            s,
            vec![ContentBlock::Text {
                text: "hello".into()
            }]
        );

        // K1 invariant: structured JSON success stays a typed Structured
        // block — never collapsed into serialized text.
        let blocks = json_output_blocks("t", &serde_json::json!({"k": 1}))
            .expect("object content should serialize");
        assert_eq!(blocks.len(), 1);
        let raw = blocks[0]
            .structured_data()
            .expect("object output must be a Structured block, not Text");
        let parsed: Value =
            serde_json::from_str(raw.get()).expect("structured payload is valid JSON");
        assert_eq!(parsed["k"], 1);
    }

    #[tokio::test]
    async fn dispatch_json_object_produces_structured_block() {
        let store = Arc::new(MemoryTaskStore::new());
        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            Some(test_project_root()),
            None,
            None,
            None,
        )
        .expect("composite dispatcher should build");

        // datetime returns a JSON object — verify it round-trips as a typed
        // Structured block through dispatch (K1 invariant: structured success
        // is never collapsed to text).
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
        assert_eq!(result.result.content.len(), 1);
        let raw = result.result.content[0]
            .structured_data()
            .expect("JSON-object tool output must arrive as a Structured block");
        let parsed: serde_json::Value =
            serde_json::from_str(raw.get()).expect("structured payload is valid JSON");
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
            Some(test_project_root()),
            None,
            Some(external),
            None,
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
    async fn dispatch_with_context_forwards_context_to_exact_external_tool() {
        let store = Arc::new(MemoryTaskStore::new());
        let external: Arc<dyn AgentToolDispatcher> =
            Arc::new(ContextAwareExactExternalDispatcher::new());
        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            Some(test_project_root()),
            None,
            Some(external),
            None,
        )
        .expect("composite dispatcher should build");

        let call_json = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "ext-ctx",
            name: "inspect_context",
            args: &call_json,
        };
        let context = ToolDispatchContext::from_current_turn_input(
            &meerkat_core::ContentInput::Blocks(vec![meerkat_core::ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "abc".into(),
            }]),
        );

        let result = dispatcher
            .dispatch_with_context(call, &context)
            .await
            .expect("external tool dispatch should succeed");
        let payload: serde_json::Value =
            serde_json::from_str(&result.result.text_content()).expect("tool result JSON");
        assert_eq!(payload["saw_context_image"], true);
    }

    #[tokio::test]
    async fn dispatch_forwards_external_tool_calls_when_allowed_set_contains_name() {
        let store = Arc::new(MemoryTaskStore::new());
        let external: Arc<dyn AgentToolDispatcher> =
            Arc::new(MockExternalDispatcher::new("mob_list", "List active mobs"));
        let mut dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            Some(test_project_root()),
            None,
            Some(external),
            None,
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

    fn tool_name_count(dispatcher: &dyn AgentToolDispatcher, name: &str) -> usize {
        dispatcher
            .tools()
            .iter()
            .filter(|tool| tool.name == name)
            .count()
    }

    fn catalog_name_count(dispatcher: &dyn AgentToolDispatcher, name: &str) -> usize {
        dispatcher
            .tool_catalog()
            .iter()
            .filter(|entry| entry.tool.name == name)
            .count()
    }

    async fn dispatch_json(
        dispatcher: &dyn AgentToolDispatcher,
        name: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        let outcome = dispatcher
            .dispatch(ToolCallView {
                id: "optional-builtin-owner",
                name,
                args: &raw,
            })
            .await
            .unwrap();
        serde_json::from_str(&outcome.result.text_content()).unwrap()
    }

    #[tokio::test]
    async fn repeated_image_registration_uses_latest_owner_before_and_after_rebind() {
        let temp_dir = TempDir::new().unwrap();
        let mut dispatcher = CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &BuiltinToolConfig::default(),
            Some(temp_dir.path().to_path_buf()),
            None,
            None,
            Some(SessionId::new().to_string()),
        )
        .unwrap();
        dispatcher.register_image_generation_tool(
            rejecting_image_runtime("stale-image-owner"),
            ToolCategoryOverride::Enable,
        );
        dispatcher.register_image_generation_tool(
            rejecting_image_runtime("latest-image-owner"),
            ToolCategoryOverride::Enable,
        );

        assert_eq!(tool_name_count(&dispatcher, "generate_image"), 1);
        assert_eq!(catalog_name_count(&dispatcher, "generate_image"), 1);
        let raw = serde_json::value::RawValue::from_string(
            json!({"request": {"prompt": "draw a dot"}}).to_string(),
        )
        .unwrap();
        let err = dispatcher
            .dispatch(ToolCallView {
                id: "image-latest-before-rebind",
                name: "generate_image",
                args: &raw,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("latest-image-owner"), "{err}");

        let registry: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new());
        let rebound = Arc::new(dispatcher)
            .bind_ops_lifecycle(registry, SessionId::new())
            .unwrap()
            .into_dispatcher();
        assert_eq!(tool_name_count(&*rebound, "generate_image"), 1);
        assert_eq!(catalog_name_count(&*rebound, "generate_image"), 1);
        let err = rebound
            .dispatch(ToolCallView {
                id: "image-latest-after-rebind",
                name: "generate_image",
                args: &raw,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("latest-image-owner"), "{err}");
    }

    #[test]
    fn latest_disabled_image_registration_removes_stale_owner_across_rebind() {
        let temp_dir = TempDir::new().unwrap();
        let mut dispatcher = CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &BuiltinToolConfig::default(),
            Some(temp_dir.path().to_path_buf()),
            None,
            None,
            Some(SessionId::new().to_string()),
        )
        .unwrap();
        dispatcher.register_image_generation_tool(
            rejecting_image_runtime("stale-image-owner"),
            ToolCategoryOverride::Enable,
        );
        dispatcher.register_image_generation_tool(
            rejecting_image_runtime("disabled-latest-owner"),
            ToolCategoryOverride::Disable,
        );

        assert_eq!(tool_name_count(&dispatcher, "generate_image"), 0);
        assert_eq!(catalog_name_count(&dispatcher, "generate_image"), 0);
        let registry: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new());
        let rebound = Arc::new(dispatcher)
            .bind_ops_lifecycle(registry, SessionId::new())
            .unwrap()
            .into_dispatcher();
        assert_eq!(tool_name_count(&*rebound, "generate_image"), 0);
        assert_eq!(catalog_name_count(&*rebound, "generate_image"), 0);
    }

    #[tokio::test]
    async fn repeated_web_search_registration_uses_latest_owner_across_rebind() {
        let temp_dir = TempDir::new().unwrap();
        let mut dispatcher = CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &BuiltinToolConfig::default(),
            Some(temp_dir.path().to_path_buf()),
            None,
            None,
            Some(SessionId::new().to_string()),
        )
        .unwrap();
        dispatcher.register_web_search_tool(
            Arc::new(MarkerWebSearchExecutor {
                marker: "stale-web-owner",
            }),
            ToolCategoryOverride::Enable,
        );
        dispatcher.register_web_search_tool(
            Arc::new(MarkerWebSearchExecutor {
                marker: "latest-web-owner",
            }),
            ToolCategoryOverride::Enable,
        );

        assert_eq!(tool_name_count(&dispatcher, "web_search"), 1);
        assert_eq!(catalog_name_count(&dispatcher, "web_search"), 1);
        let result = dispatch_json(&dispatcher, "web_search", json!({"query": "owner"})).await;
        assert_eq!(result["answer"], "latest-web-owner");

        let registry: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new());
        let rebound = Arc::new(dispatcher)
            .bind_ops_lifecycle(registry, SessionId::new())
            .unwrap()
            .into_dispatcher();
        assert_eq!(tool_name_count(&*rebound, "web_search"), 1);
        assert_eq!(catalog_name_count(&*rebound, "web_search"), 1);
        let result = dispatch_json(&*rebound, "web_search", json!({"query": "owner"})).await;
        assert_eq!(result["answer"], "latest-web-owner");
    }

    #[test]
    fn latest_disabled_web_search_registration_removes_stale_owner_across_rebind() {
        let temp_dir = TempDir::new().unwrap();
        let mut dispatcher = CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &BuiltinToolConfig::default(),
            Some(temp_dir.path().to_path_buf()),
            None,
            None,
            Some(SessionId::new().to_string()),
        )
        .unwrap();
        dispatcher.register_web_search_tool(
            Arc::new(MarkerWebSearchExecutor {
                marker: "stale-web-owner",
            }),
            ToolCategoryOverride::Enable,
        );
        dispatcher.register_web_search_tool(
            Arc::new(MarkerWebSearchExecutor {
                marker: "disabled-latest-owner",
            }),
            ToolCategoryOverride::Disable,
        );

        assert_eq!(tool_name_count(&dispatcher, "web_search"), 0);
        assert_eq!(catalog_name_count(&dispatcher, "web_search"), 0);
        let registry: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new());
        let rebound = Arc::new(dispatcher)
            .bind_ops_lifecycle(registry, SessionId::new())
            .unwrap()
            .into_dispatcher();
        assert_eq!(tool_name_count(&*rebound, "web_search"), 0);
        assert_eq!(catalog_name_count(&*rebound, "web_search"), 0);
    }

    #[test]
    fn blob_file_tools_register_when_blob_store_is_wired() {
        let temp_dir = TempDir::new().unwrap();
        let mut dispatcher = CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &BuiltinToolConfig::default(),
            Some(temp_dir.path().to_path_buf()),
            None,
            None,
            None,
        )
        .expect("composite dispatcher should build");

        dispatcher.register_blob_file_tools(Arc::new(TestBlobStore::default()));

        let names: Vec<_> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        for expected in ["blob_save_file", "blob_load_file", "blob_inspect"] {
            assert!(
                names.contains(&expected.to_string()),
                "{expected} should be exposed when a blob store is wired; tools={names:?}"
            );
        }
    }

    #[tokio::test]
    async fn blob_file_tools_are_absent_without_blob_store() {
        let temp_dir = TempDir::new().unwrap();
        let dispatcher = CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &BuiltinToolConfig::default(),
            Some(temp_dir.path().to_path_buf()),
            None,
            None,
            None,
        )
        .expect("composite dispatcher should build");

        assert!(
            dispatcher
                .tools()
                .iter()
                .all(|tool| !tool.name.starts_with("blob_")),
            "blob tools should not be advertised without a session blob store"
        );

        let call_json =
            serde_json::value::RawValue::from_string(r#"{"blob_id":"sha256:missing"}"#.into())
                .unwrap();
        let call = ToolCallView {
            id: "blob-inspect",
            name: "blob_inspect",
            args: &call_json,
        };
        let err = dispatcher.dispatch(call).await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound { .. }));
    }

    #[tokio::test]
    async fn blob_file_tools_respect_builtin_policy() {
        let temp_dir = TempDir::new().unwrap();
        let config = BuiltinToolConfig {
            policy: ToolPolicyLayer::new().disable_tool("blob_save_file"),
            ..Default::default()
        };
        let mut dispatcher = CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &config,
            Some(temp_dir.path().to_path_buf()),
            None,
            None,
            None,
        )
        .expect("composite dispatcher should build");

        let store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let blob_ref = store.put_image("image/png", "iVBORw0KGgo=").await.unwrap();
        dispatcher.register_blob_file_tools(store);

        assert!(
            dispatcher
                .tools()
                .iter()
                .all(|tool| tool.name != "blob_save_file"),
            "disabled blob_save_file must not be advertised"
        );
        assert!(
            dispatcher
                .tools()
                .iter()
                .any(|tool| tool.name == "blob_inspect"),
            "other blob tools should remain available"
        );

        let call_json = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "blob_id": blob_ref.blob_id.as_str(),
                "path": "out.png",
            })
            .to_string(),
        )
        .unwrap();
        let call = ToolCallView {
            id: "blob-save",
            name: "blob_save_file",
            args: &call_json,
        };
        let err = dispatcher.dispatch(call).await.unwrap_err();
        assert!(matches!(err, ToolError::AccessDenied { .. }));
    }

    #[tokio::test]
    async fn blob_file_tools_survive_ops_lifecycle_rebind() {
        let temp_dir = TempDir::new().unwrap();
        let store: Arc<dyn BlobStore> = Arc::new(TestBlobStore::default());
        let mut dispatcher = CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &BuiltinToolConfig::default(),
            Some(temp_dir.path().to_path_buf()),
            None,
            None,
            Some(SessionId::new().to_string()),
        )
        .expect("composite dispatcher should build");
        dispatcher.register_blob_file_tools(store);

        let registry: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new());
        let rebound = Arc::new(dispatcher)
            .bind_ops_lifecycle(registry, SessionId::new())
            .expect("ops lifecycle binding should preserve blob tools")
            .into_dispatcher();

        let names: Vec<_> = rebound
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        for expected in ["blob_save_file", "blob_load_file", "blob_inspect"] {
            assert!(
                names.contains(&expected.to_string()),
                "{expected} should survive ops lifecycle rebinding; tools={names:?}"
            );
        }
    }

    #[tokio::test]
    async fn repeated_blob_registration_uses_latest_owner_before_and_after_rebind() {
        let temp_dir = TempDir::new().unwrap();
        let shared_id = BlobId::new("blob-shared-owner-test");
        let stale_store = Arc::new(TestBlobStore::default());
        stale_store.blobs.lock().await.insert(
            shared_id.clone(),
            BlobPayload {
                blob_id: shared_id.clone(),
                media_type: "application/x-stale-owner".to_string(),
                data: "YQ==".to_string(),
            },
        );
        let latest_store = Arc::new(TestBlobStore::default());
        latest_store.blobs.lock().await.insert(
            shared_id.clone(),
            BlobPayload {
                blob_id: shared_id.clone(),
                media_type: "application/x-latest-owner".to_string(),
                data: "YmJi".to_string(),
            },
        );
        let mut dispatcher = CompositeDispatcher::new(
            Arc::new(MemoryTaskStore::new()),
            &BuiltinToolConfig::default(),
            Some(temp_dir.path().to_path_buf()),
            None,
            None,
            Some(SessionId::new().to_string()),
        )
        .unwrap();
        dispatcher.register_blob_file_tools(stale_store);
        dispatcher.register_blob_file_tools(latest_store);

        for name in BLOB_FILE_TOOL_NAMES {
            assert_eq!(tool_name_count(&dispatcher, name), 1, "{name}");
            assert_eq!(catalog_name_count(&dispatcher, name), 1, "{name}");
        }
        let result = dispatch_json(
            &dispatcher,
            "blob_inspect",
            json!({"blob_id": shared_id.as_str()}),
        )
        .await;
        assert_eq!(result["media_type"], "application/x-latest-owner");
        assert_eq!(result["size_bytes"], 3);

        let registry: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new());
        let rebound = Arc::new(dispatcher)
            .bind_ops_lifecycle(registry, SessionId::new())
            .unwrap()
            .into_dispatcher();
        for name in BLOB_FILE_TOOL_NAMES {
            assert_eq!(tool_name_count(&*rebound, name), 1, "{name}");
            assert_eq!(catalog_name_count(&*rebound, name), 1, "{name}");
        }
        let result = dispatch_json(
            &*rebound,
            "blob_inspect",
            json!({"blob_id": shared_id.as_str()}),
        )
        .await;
        assert_eq!(result["media_type"], "application/x-latest-owner");
        assert_eq!(result["size_bytes"], 3);
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
        // view_image is always registered. Visibility gating is handled at the
        // factory/ToolScope level via external filters.
        let store = Arc::new(MemoryTaskStore::new());
        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            Some(test_project_root()),
            None,
            None,
            None,
        )
        .expect("composite dispatcher should build");

        let tools = dispatcher.tools();
        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        assert!(
            tool_names.contains(&"view_image".to_string()),
            "view_image should always be in base tool set, but found: {tool_names:?}"
        );
    }

    #[test]
    #[allow(clippy::panic)]
    fn builtin_tools_have_correct_provenance() {
        use meerkat_core::types::ToolSourceKind;

        let store: Arc<dyn crate::builtin::TaskStore> = Arc::new(MemoryTaskStore::new());

        let dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            Some(test_project_root()),
            None,
            None,
            None,
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

    // Minimal SkillEngine test double. The skill tool registration path only
    // reads each tool's `name()` / `default_enabled()`, so the engine methods
    // below are never invoked; they exist only to satisfy the trait bound.
    #[cfg(feature = "skills")]
    fn stub_skill_key() -> meerkat_core::skills::SkillKey {
        meerkat_core::skills::SkillKey::new(
            meerkat_core::skills::SourceUuid::from_uuid(uuid::Uuid::nil()),
            meerkat_core::skills::SkillName::parse("stub").unwrap(),
        )
    }

    #[cfg(feature = "skills")]
    struct StubSkillEngine;

    #[cfg(feature = "skills")]
    impl meerkat_core::skills::SkillEngine for StubSkillEngine {
        async fn inventory_section(&self) -> Result<String, meerkat_core::skills::SkillError> {
            Ok(String::new())
        }

        async fn resolve_and_render(
            &self,
            _keys: &[meerkat_core::skills::SkillKey],
        ) -> Result<Vec<meerkat_core::skills::ResolvedSkill>, meerkat_core::skills::SkillError>
        {
            Ok(Vec::new())
        }

        async fn collections(
            &self,
        ) -> Result<Vec<meerkat_core::skills::SkillCollection>, meerkat_core::skills::SkillError>
        {
            Ok(Vec::new())
        }

        async fn list_skills(
            &self,
            _filter: &meerkat_core::skills::SkillFilter,
        ) -> Result<Vec<meerkat_core::skills::SkillDescriptor>, meerkat_core::skills::SkillError>
        {
            Ok(Vec::new())
        }

        async fn quarantined_diagnostics(
            &self,
        ) -> Result<
            Vec<meerkat_core::skills::SkillQuarantineDiagnostic>,
            meerkat_core::skills::SkillError,
        > {
            Ok(Vec::new())
        }

        async fn health_snapshot(
            &self,
        ) -> Result<meerkat_core::skills::SourceHealthSnapshot, meerkat_core::skills::SkillError>
        {
            Err(meerkat_core::skills::SkillError::NotFound {
                key: stub_skill_key(),
            })
        }

        async fn list_artifacts(
            &self,
            _key: &meerkat_core::skills::SkillKey,
        ) -> Result<Vec<meerkat_core::skills::SkillArtifact>, meerkat_core::skills::SkillError>
        {
            Ok(Vec::new())
        }

        async fn read_artifact(
            &self,
            key: &meerkat_core::skills::SkillKey,
            _artifact_path: &str,
        ) -> Result<meerkat_core::skills::SkillArtifactContent, meerkat_core::skills::SkillError>
        {
            Err(meerkat_core::skills::SkillError::NotFound { key: key.clone() })
        }

        async fn invoke_function(
            &self,
            key: &meerkat_core::skills::SkillKey,
            _function_name: &meerkat_core::skills::SkillFunctionName,
            _arguments: meerkat_core::ToolCallArguments,
        ) -> Result<meerkat_core::skills::SkillFunctionOutput, meerkat_core::skills::SkillError>
        {
            Err(meerkat_core::skills::SkillError::NotFound { key: key.clone() })
        }
    }

    #[cfg(feature = "skills")]
    fn stub_skill_tool_set() -> SkillToolSet {
        let runtime = Arc::new(meerkat_core::skills::SkillRuntime::new(Arc::new(
            StubSkillEngine,
        )));
        SkillToolSet::new(runtime)
    }

    #[cfg(feature = "skills")]
    #[test]
    fn register_skill_tools_respects_default_disabled_policy() {
        // Dogma row #318: skill builtins are default_enabled() == false and must NOT
        // appear in the catalog under the default (AllowAll) policy.
        let store: Arc<dyn crate::builtin::TaskStore> = Arc::new(MemoryTaskStore::new());
        let mut dispatcher = CompositeDispatcher::new(
            store,
            &BuiltinToolConfig::default(),
            Some(test_project_root()),
            None,
            None,
            None,
        )
        .unwrap();

        dispatcher.register_skill_tools(stub_skill_tool_set());

        let names: Vec<String> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert!(
            !names.contains(&"browse_skills".to_string()),
            "browse_skills must stay disabled by default: {names:?}"
        );
        assert!(
            !names.contains(&"load_skill".to_string()),
            "load_skill must stay disabled by default: {names:?}"
        );
    }

    #[cfg(feature = "skills")]
    #[test]
    fn register_skill_tools_honors_explicit_enable() {
        // With explicit policy enable, the skill tools must appear in the catalog.
        let store: Arc<dyn crate::builtin::TaskStore> = Arc::new(MemoryTaskStore::new());
        let config = BuiltinToolConfig {
            policy: ToolPolicyLayer::new()
                .enable_tool("browse_skills")
                .enable_tool("load_skill"),
            ..BuiltinToolConfig::default()
        };
        let mut dispatcher =
            CompositeDispatcher::new(store, &config, Some(test_project_root()), None, None, None)
                .unwrap();

        dispatcher.register_skill_tools(stub_skill_tool_set());

        let names: Vec<String> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert!(
            names.contains(&"browse_skills".to_string()),
            "explicitly enabled browse_skills must appear: {names:?}"
        );
        assert!(
            names.contains(&"load_skill".to_string()),
            "explicitly enabled load_skill must appear: {names:?}"
        );
    }
}
