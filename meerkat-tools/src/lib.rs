#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! meerkat-tools - Tool validation and dispatch for Meerkat
//!
//! This crate provides tool registry and dispatch functionality.
//!
//! ## Built-in Tools
//!
//! The [`builtin`] module provides built-in tools for task management and utilities.
//! Use [`CompositeDispatcher`] to combine built-in tools with external MCP tools.
//!
//! ```text
//! use meerkat_tools::{
//!     CompositeDispatcher, BuiltinToolConfig, FileTaskStore,
//!     find_project_root, ensure_rkat_dir,
//! };
//!
//! let project_root = find_project_root(&std::env::current_dir().unwrap())
//!     .expect("no .rkat directory found");
//! ensure_rkat_dir(&project_root).unwrap();
//! let store = Arc::new(FileTaskStore::in_project(&project_root));
//! let dispatcher = CompositeDispatcher::new(
//!     store,
//!     &BuiltinToolConfig::default(),
//!     Some(project_root),
//!     None,
//!     None,
//!     None,
//! )?;
//! ```

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

#[cfg(not(target_arch = "wasm32"))]
pub mod builder;
pub mod builtin;
pub mod control_plane;
pub mod dispatcher;
pub mod error;
pub mod registry;
pub mod schema;
pub mod timeout;

#[cfg(all(feature = "comms", not(target_arch = "wasm32")))]
pub use builder::CommsDispatcherConfig;
#[cfg(all(feature = "mcp", not(target_arch = "wasm32")))]
pub use builder::McpDispatcherConfig;
#[cfg(not(target_arch = "wasm32"))]
pub use builder::{BuiltinDispatcherConfig, ToolDispatcherBuilder, build_builtin_dispatcher};
#[cfg(feature = "comms")]
pub use builtin::CommsToolSurface;
pub use builtin::{
    BuiltinTool, BuiltinToolConfig, BuiltinToolEntry, BuiltinToolError, CompositeDispatcher,
    CompositeDispatcherError, EnforcedToolPolicy, MemoryTaskStore, ResolvedToolPolicy, TaskStore,
    ToolMode, ToolPolicyLayer,
};
#[cfg(not(target_arch = "wasm32"))]
pub use builtin::{FileTaskStore, ensure_rkat_dir, ensure_rkat_dir_async, find_project_root};
pub use control_plane::{CatalogControlDispatcher, CatalogControlVisibilityProvider};
pub use dispatcher::EmptyToolDispatcher;
#[cfg(not(target_arch = "wasm32"))]
pub use dispatcher::ToolDispatcher;
pub use error::{DispatchError, ToolError, ToolValidationError};
#[cfg(feature = "comms")]
pub use meerkat_comms::agent::{CommsToolDispatcher, DynCommsToolDispatcher, NoOpDispatcher};
pub use registry::validate_tool_def;
pub use schema::{empty_object_schema, schema_for};
pub use timeout::ToolTimeoutPolicy;

// Capability registrations
inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::Builtins,
        description: "Built-in tools: task_list, task_create, task_get, task_update, datetime, apply_patch, view_image, generate_image, blob_save_file, blob_load_file, blob_inspect",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: Some(|config| {
            if config.tools.builtins_enabled {
                meerkat_capabilities::CapabilityStatus::Available
            } else {
                meerkat_capabilities::CapabilityStatus::DisabledByPolicy {
                    description: std::borrow::Cow::Borrowed("tools.builtins_enabled = false"),
                }
            }
        }),
    }
}

#[cfg(not(target_arch = "wasm32"))]
inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::Shell,
        description: "Shell tool with job management: shell, shell_jobs, shell_job_status, shell_job_cancel",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: Some(|config| {
            if config.tools.shell_enabled {
                meerkat_capabilities::CapabilityStatus::Available
            } else {
                meerkat_capabilities::CapabilityStatus::DisabledByPolicy {
                    description: std::borrow::Cow::Borrowed("tools.shell_enabled = false"),
                }
            }
        }),
    }
}

// Skill registrations
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "task-workflow",
        name: "Task Workflow",
        description: "How to use task_create/task_update/task_list for structured work tracking",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["builtins"],
        body: include_str!("../skills/task-workflow/SKILL.md"),
        extensions: &[],
    }
}

inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "skill-discovery-workflow",
        name: "Skill Discovery Workflow",
        description: "How to browse, load, and use Meerkat skills during a session",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["skills"],
        body: include_str!("../skills/skill-discovery-workflow/SKILL.md"),
        extensions: &[],
    }
}

inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "builtin-utilities-workflow",
        name: "Builtin Utilities Workflow",
        description: "How to use Meerkat built-in utility tools safely and productively",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["builtins"],
        body: include_str!("../skills/builtin-utilities-workflow/SKILL.md"),
        extensions: &[],
    }
}

#[cfg(not(target_arch = "wasm32"))]
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "shell-patterns",
        name: "Shell Patterns",
        description: "Background job patterns with shell and job management tools",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["builtins", "shell"],
        body: include_str!("../skills/shell-patterns/SKILL.md"),
        extensions: &[],
    }
}
