//! Utility tools for general-purpose agent operations
//!
//! This module provides utility tools that are useful across many agent workflows:
//! - [`WaitTool`] - Pause execution for a specified duration
//! - [`DateTimeTool`] - Get the current date and time
//!
//! These tools are enabled by default when built-in tools are enabled.

mod datetime;
mod tool_set;
mod wait;

pub use datetime::DateTimeTool;
pub use tool_set::UtilityToolSet;
pub use wait::{WaitInterrupt, WaitTool};
