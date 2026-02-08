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

pub use builder::{BuiltinDispatcherConfig, ToolDispatcherBuilder, build_builtin_dispatcher};
#[cfg(feature = "comms")]
pub use builder::CommsDispatcherConfig;
#[cfg(feature = "mcp")]
pub use builder::McpDispatcherConfig;
pub use builtin::{
    BuiltinTool, BuiltinToolConfig, BuiltinToolEntry, BuiltinToolError, CompositeDispatcher,
    CompositeDispatcherError, EnforcedToolPolicy, FileTaskStore, MemoryTaskStore,
    ResolvedToolPolicy, TaskStore, ToolMode, ToolPolicyLayer, ensure_rkat_dir,
    ensure_rkat_dir_async, find_project_root,
};
#[cfg(feature = "comms")]
pub use builtin::CommsToolSurface;
pub use dispatcher::{EmptyToolDispatcher, FilteredDispatcher, ToolDispatcher};
pub use error::{DispatchError, ToolError, ToolValidationError};
#[cfg(feature = "comms")]
pub use meerkat_comms::agent::{
    CommsToolDispatcher, DynCommsToolDispatcher, NoOpDispatcher, wrap_with_comms,
};
pub use registry::ToolRegistry;
pub use schema::{empty_object_schema, schema_for};
