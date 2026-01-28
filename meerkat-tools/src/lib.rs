//! meerkat-tools - Tool validation and dispatch for Meerkat
//!
//! This crate provides tool registry and dispatch functionality.
//!
//! ## Built-in Tools
//!
//! The [`builtin`] module provides built-in tools for task management and utilities.
//! Use [`CompositeDispatcher`] to combine built-in tools with external MCP tools.
//!
//! ```ignore
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
pub mod comms_dispatcher;
pub mod dispatcher;
pub mod error;
pub mod registry;
pub mod schema;

pub use builder::{
    BuiltinDispatcherConfig, CommsDispatcherConfig, DispatcherBuildError, McpDispatcherConfig,
    ToolDispatcherBuilder, build_builtin_dispatcher,
};
pub use builtin::{
    BuiltinTool, BuiltinToolConfig, BuiltinToolEntry, BuiltinToolError, CommsToolSurface,
    CompositeDispatcher, CompositeDispatcherError, EnforcedToolPolicy, FileTaskStore,
    MemoryTaskStore, ResolvedToolPolicy, TaskStore, ToolMode, ToolPolicyLayer, ensure_rkat_dir,
    find_project_root,
};
pub use comms_dispatcher::{
    CommsToolDispatcher, DynCommsToolDispatcher, NoOpDispatcher, wrap_with_comms,
};
pub use dispatcher::EmptyToolDispatcher;
pub use dispatcher::{FilteredToolDispatcher, ToolDispatcher};
pub use error::{DispatchError, ToolError, ToolValidationError};
pub use registry::ToolRegistry;
pub use schema::{SchemaBuilder, empty_object_schema};
