//! meerkat-tools - Tool validation and dispatch for Meerkat
//!
//! This crate provides tool registry and dispatch functionality.

pub mod dispatcher;
pub mod error;
pub mod registry;

pub use dispatcher::ToolDispatcher;
pub use error::{DispatchError, ToolError, ToolValidationError};
pub use registry::ToolRegistry;
