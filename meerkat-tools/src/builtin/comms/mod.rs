//! Comms tools as a separate tool surface
//!
//! Comms is infrastructure, not a tool category. These tools enable inter-agent
//! communication and are composed with other dispatchers via [`ToolGateway`],
//! NOT bundled into [`CompositeDispatcher`].
//!
//! # Usage
//!
//! ```ignore
//! use meerkat_core::ToolGateway;
//! use meerkat_tools::builtin::comms::CommsToolSurface;
//!
//! // Create comms surface from runtime
//! let comms_surface = CommsToolSurface::new(
//!     comms_runtime.router_arc(),
//!     comms_runtime.trusted_peers_shared(),
//! );
//!
//! // Compose with base dispatcher
//! let gateway = ToolGateway::new(base_dispatcher, Some(Arc::new(comms_surface)))?;
//! ```

mod surface;
mod tool_set;
mod tools;

pub use surface::CommsToolSurface;
pub use tool_set::CommsToolSet;
pub use tools::{ListPeersTool, SendMessageTool, SendRequestTool, SendResponseTool};
