//! Inert package metadata and exact, validated activation selections.
//!
//! Agents interpret user requests and select integrations. This crate validates
//! those concrete selections; it does not resolve intent, execute package code,
//! classify operations as trusted, grant access, or establish runtime readiness.

mod agent_plugins;
mod catalog;
mod compose;
mod discovery;
mod gitops;
mod library;
mod local_install;
mod local_software;
#[cfg(feature = "local-recipes")]
mod local_recipes;
mod manifest;
mod mcpb;
mod mcpb_install;
mod model;
mod node_install;
mod python_install;
mod recipe_inventory;
mod remote_export;
mod value;

pub use agent_plugins::*;
pub use catalog::*;
pub use compose::*;
pub use discovery::*;
pub use gitops::*;
pub use library::*;
pub use local_software::*;
#[cfg(feature = "local-recipes")]
pub use local_recipes::*;
pub use manifest::*;
pub use mcpb::*;
pub use mcpb_install::*;
pub use model::*;
pub use node_install::*;
pub use python_install::*;
pub use recipe_inventory::*;
pub use remote_export::*;
pub use value::*;

/// Local byte preparation only; no execution, activation or authorization.
#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
pub mod local_artifacts;
