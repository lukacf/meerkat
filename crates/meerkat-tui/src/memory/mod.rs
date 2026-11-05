//! Notes: small Markdown files the agent saves for later sessions (distinct
//! from the engine's native semantic memory, `memory_search`).
//!
//! Two scopes exist: user-level notes in `<config>/notes/` (durable personal
//! preferences that follow the user into every project) and workspace notes in
//! `<workspace>/.rkat/tui/notes/` (facts about this project). Each directory
//! holds one `<slug>.md` per note plus a regenerated `MEMORY.md` index.
//!
//! - [`store`] — [`NoteStore`]: slugging, caps, atomic owner-only writes,
//!   symlink refusal below the trusted root, index generation and the
//!   untrusted system-prompt section ([`prompt_index`]).
//! - [`tools`] — [`NoteToolDispatcher`]: `note_save`, `note_read`,
//!   `note_forget`, `note_list`.

pub mod store;
pub mod tools;

pub use store::{
    INDEX_FILE, MAX_NOTES, NOTE_MAX_BYTES, NoteError, NoteStore, NoteSummary,
    PROMPT_INDEX_MAX_BYTES, Scope, UNTRUSTED_INDEX_LABEL, UNTRUSTED_INDEX_WARNING, USER_NOTES_REL,
    WORKSPACE_NOTES_REL, prompt_index, slug,
};
pub use tools::{
    NOTE_FORGET, NOTE_LIST, NOTE_READ, NOTE_SAVE, NOTE_TOOL_NAMES, NOTES_SOURCE, NoteToolDispatcher,
};

/// Transitional alias; remove once callers use the `NOTE_*` names.
pub const MEMORY_SAVE: &str = NOTE_SAVE;
/// Transitional alias; remove once callers use the `NOTE_*` names.
pub const MEMORY_READ: &str = NOTE_READ;
/// Transitional alias; remove once callers use the `NOTE_*` names.
pub const MEMORY_FORGET: &str = NOTE_FORGET;
/// Transitional alias; remove once callers use the `NOTE_*` names.
pub const MEMORY_LIST: &str = NOTE_LIST;
/// Transitional alias; remove once callers use the `NOTE_*` names.
pub const MEMORY_TOOL_NAMES: &[&str] = NOTE_TOOL_NAMES;
/// Transitional alias; remove once callers use the `NoteToolDispatcher` name.
pub type MemoryToolDispatcher = NoteToolDispatcher;
