//! Utility tools for general-purpose agent operations
//!
//! This module provides utility tools that are useful across many agent workflows:
//! - [`WaitTool`] - Pause execution for a specified duration
//! - [`DateTimeTool`] - Get the current date and time
//! - [`ApplyPatchTool`] - Apply structured file edits inside the project root
//!
//! These tools are enabled by default when built-in tools are enabled.

#[cfg(not(target_arch = "wasm32"))]
mod apply_patch;
mod datetime;
mod tool_set;
#[cfg(not(target_arch = "wasm32"))]
mod view_image;
mod wait;

#[cfg(not(target_arch = "wasm32"))]
pub use apply_patch::ApplyPatchTool;
pub use datetime::DateTimeTool;
pub use tool_set::UtilityToolSet;
#[cfg(not(target_arch = "wasm32"))]
pub use view_image::ViewImageTool;
pub use wait::{WaitInterrupt, WaitTool};
