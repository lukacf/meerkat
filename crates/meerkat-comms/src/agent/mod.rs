//! Agent-facing comms tool dispatch and message projection types.

pub mod dispatcher;
pub mod types;

pub use dispatcher::{CommsToolDispatcher, DynCommsToolDispatcher, NoOpDispatcher};
pub use types::{CommsContent, CommsMessage, CommsStatus, MessageIntent};
