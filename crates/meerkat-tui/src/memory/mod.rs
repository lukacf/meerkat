//! Memory notes: small Markdown files the agent saves for later sessions.
//!
//! Two scopes exist: user-level notes in `<config>/notes/` (durable personal
//! preferences that follow the user into every project) and workspace notes in
//! `<workspace>/.rkat/tui/notes/` (facts about this project). Each directory
//! holds one `<slug>.md` per note plus a regenerated `MEMORY.md` index.
//!
//! - [`store`] — [`NoteStore`]: slugging, caps, atomic writes, symlink refusal,
//!   index generation and the system-prompt index ([`prompt_index`]).
//! - [`tools`] — [`MemoryToolDispatcher`]: `memory_save`, `memory_read`,
//!   `memory_forget`, `memory_list`.

pub mod store;
pub mod tools;

pub use store::{
    INDEX_FILE, MAX_NOTES, NOTE_MAX_BYTES, NoteError, NoteStore, NoteSummary,
    PROMPT_INDEX_MAX_BYTES, Scope, prompt_index, slug,
};
pub use tools::{
    MEMORY_FORGET, MEMORY_LIST, MEMORY_READ, MEMORY_SAVE, MEMORY_TOOL_NAMES, MemoryToolDispatcher,
};
