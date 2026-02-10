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
//! let dispatcher = CompositeDispatcher::new(store, &BuiltinToolConfig::default(), None, None)?;
//! ```

pub mod builder;
pub mod builtin;
pub mod dispatcher;
pub mod error;
pub mod registry;
pub mod schema;

#[cfg(feature = "comms")]
pub use builder::CommsDispatcherConfig;
#[cfg(feature = "mcp")]
pub use builder::McpDispatcherConfig;
pub use builder::{BuiltinDispatcherConfig, ToolDispatcherBuilder, build_builtin_dispatcher};
#[cfg(feature = "comms")]
pub use builtin::CommsToolSurface;
pub use builtin::{
    BuiltinTool, BuiltinToolConfig, BuiltinToolEntry, BuiltinToolError, CompositeDispatcher,
    CompositeDispatcherError, EnforcedToolPolicy, FileTaskStore, MemoryTaskStore,
    ResolvedToolPolicy, TaskStore, ToolMode, ToolPolicyLayer, ensure_rkat_dir,
    ensure_rkat_dir_async, find_project_root,
};
pub use dispatcher::{EmptyToolDispatcher, FilteredDispatcher, ToolDispatcher};
pub use error::{DispatchError, ToolError, ToolValidationError};
#[cfg(feature = "comms")]
pub use meerkat_comms::agent::{
    CommsToolDispatcher, DynCommsToolDispatcher, NoOpDispatcher, wrap_with_comms,
};
pub use registry::ToolRegistry;
pub use schema::{empty_object_schema, schema_for};

// Capability registrations
inventory::submit! {
    meerkat_contracts::CapabilityRegistration {
        id: meerkat_contracts::CapabilityId::Builtins,
        description: "Built-in tools: task_list, task_create, task_get, task_update, wait, datetime",
        scope: meerkat_contracts::CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: Some(|config| {
            if config.tools.builtins_enabled {
                meerkat_contracts::CapabilityStatus::Available
            } else {
                meerkat_contracts::CapabilityStatus::DisabledByPolicy {
                    description: std::borrow::Cow::Borrowed("tools.builtins_enabled = false"),
                }
            }
        }),
    }
}

inventory::submit! {
    meerkat_contracts::CapabilityRegistration {
        id: meerkat_contracts::CapabilityId::Shell,
        description: "Shell tool with job management: shell, shell_jobs, shell_job_status, shell_job_cancel",
        scope: meerkat_contracts::CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: Some(|config| {
            if config.tools.shell_enabled {
                meerkat_contracts::CapabilityStatus::Available
            } else {
                meerkat_contracts::CapabilityStatus::DisabledByPolicy {
                    description: std::borrow::Cow::Borrowed("tools.shell_enabled = false"),
                }
            }
        }),
    }
}

#[cfg(feature = "sub-agents")]
inventory::submit! {
    meerkat_contracts::CapabilityRegistration {
        id: meerkat_contracts::CapabilityId::SubAgents,
        description: "Sub-agent management: agent_spawn, agent_fork, agent_status, agent_cancel, agent_list",
        scope: meerkat_contracts::CapabilityScope::Universal,
        requires_feature: Some("sub-agents"),
        prerequisites: &[],
        status_resolver: None,
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
        id: "shell-patterns",
        name: "Shell Patterns",
        description: "Background job patterns with shell and job management tools",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["builtins", "shell"],
        body: include_str!("../skills/shell-patterns/SKILL.md"),
        extensions: &[],
    }
}

#[cfg(feature = "sub-agents")]
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "sub-agent-orchestration",
        name: "Sub-Agent Orchestration",
        description: "When to agent_spawn vs agent_fork, model selection, result aggregation",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["sub_agents"],
        body: include_str!("../skills/sub-agent-orchestration/SKILL.md"),
        extensions: &[],
    }
}
