//! Utility tools for general-purpose agent operations
//!
//! This module provides utility tools that are useful across many agent workflows:
//! - [`DateTimeTool`] - Get the current date and time
//! - [`ApplyPatchTool`] - Apply structured file edits inside the project root
//! - Blob file tools - Move files to and from the session blob store
//!
//! These tools are enabled by default when built-in tools are enabled.

#[cfg(not(target_arch = "wasm32"))]
mod apply_patch;
#[cfg(not(target_arch = "wasm32"))]
mod blob_file;
mod datetime;
mod tool_set;
#[cfg(not(target_arch = "wasm32"))]
mod view_image;

#[cfg(not(target_arch = "wasm32"))]
pub use apply_patch::ApplyPatchTool;
#[cfg(not(target_arch = "wasm32"))]
pub use blob_file::{BlobInspectTool, BlobLoadFileTool, BlobSaveFileTool};
pub use datetime::DateTimeTool;
pub use tool_set::UtilityToolSet;
#[cfg(not(target_arch = "wasm32"))]
pub use view_image::ViewImageTool;
