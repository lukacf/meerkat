//! The `fork` tool: hand a side quest to a child that starts with the
//! calling agent's own conversation as context.
//!
//! The engine's `fork_off` is denied to every TUI session and helper (see
//! [`crate::session::UNGATED_SPAWN_TOOLS`]): its child seats through the
//! engine's persisted-fork path, which never consults the TUI's spawn
//! customizer, and it refuses a caller that is not a mob member. A durable
//! fork of the caller's own session is no way around it either: called from
//! inside the caller's running turn, the engine's `Quiescent` fork refuses
//! the session as busy, and its `CallerTurn` fork needs a mob member
//! binding for the child.
//!
//! `fork` therefore runs the child as a helper of the caller, exactly like
//! `delegate` does (a member of the caller's implicit mob, one turn, then
//! retired), so the child passes the caller's approval gate and is followed,
//! counted, costed and interrupted like every helper (see [`crate::mobs`]).
//! What makes it a fork is its prompt: the caller's committed conversation,
//! rendered as text ([`context`]), then the task. Only the main session's
//! build carries the tool; a helper (and so a fork child) never does, so
//! forks do not nest.

pub mod context;
#[cfg(feature = "mob")]
mod dispatcher;

#[cfg(feature = "mob")]
pub use dispatcher::ForkToolDispatcher;

/// Tool name.
pub const FORK: &str = "fork";

/// Provenance source id of the `fork` tool (see
/// [`crate::tools::FILE_TOOLS_SOURCE`]): the approval policy treats `fork`
/// as delegation only when the session's tool of that name comes from here.
pub const FORK_SOURCE: &str = "rkat-tui.fork";

/// Member identity prefix of a fork child (`fork-1`, `fork-2`, ...): the
/// label the transcript, the status bar, `/cost` and the approval notices
/// show.
pub const FORK_LABEL_PREFIX: &str = "fork-";

/// Most bytes of the child's final answer returned as the tool result
/// (an explicit truncation marker included).
pub const MAX_RESULT_BYTES: usize = 16 * 1024;
