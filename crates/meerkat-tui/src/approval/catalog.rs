//! Where a session's tools come from, as the approval policy sees it.
//!
//! A tool's name says nothing about who implements it: an MCP server may
//! offer a tool called `read_file` or `shell`. The tier policy therefore
//! treats a name as a builtin, a file tool or a note tool only when the
//! session's live tool definitions say so ([`ToolProvenance`]):
//!
//! - engine-native provenance (`builtin`, `shell`, `comms`, `memory`,
//!   `schedule`, `workgraph`, `mob`) ⇒ [`ToolFamily::Native`];
//! - the TUI's own callback surfaces ([`FILE_TOOLS_SOURCE`],
//!   [`NOTES_SOURCE`], [`FORK_SOURCE`]) ⇒ [`ToolFamily::Files`] /
//!   [`ToolFamily::Notes`] / [`ToolFamily::Fork`];
//! - MCP, Rust bundles, any other callback surface, a definition without
//!   provenance and a name the session does not offer ⇒
//!   [`ToolFamily::Foreign`], which the policy always sends to the
//!   classifier unless a user rule decides it.
//!
//! The gate learns a session's definitions from a [`ToolOrigins`] source at
//! every call (the runtime's published visible tool definitions), so a tool
//! that appears or is replaced mid-session is judged by what it is now.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use meerkat_core::{SessionId, ToolDef, ToolMutationClass, ToolProvenance, ToolSourceKind};

use crate::fork::FORK_SOURCE;
use crate::memory::{NOTE_FORGET, NOTE_LIST, NOTE_READ, NOTE_SAVE, NOTES_SOURCE};
use crate::tools::{EDIT_FILE, FILE_TOOLS_SOURCE, GLOB, GREP, READ_FILE, WRITE_FILE};

/// Which implementation family a tool belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolFamily {
    /// An engine builtin (native provenance).
    Native,
    /// The TUI's file tools.
    Files,
    /// The TUI's note tools.
    Notes,
    /// The TUI's `fork` tool ([`crate::fork`]).
    Fork,
    /// Anything else: MCP, bundles, other callback surfaces, no provenance,
    /// or a name the session does not offer.
    Foreign,
}

/// Where one tool comes from and what it declares about mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOrigin {
    /// The definition's provenance, if it carries one.
    pub provenance: Option<ToolProvenance>,
    /// The tool's declared mutation class (`Unknown` when undeclared).
    pub mutation: ToolMutationClass,
}

impl ToolOrigin {
    /// An engine-native tool of `kind`.
    #[must_use]
    pub fn native(kind: ToolSourceKind) -> Self {
        Self {
            provenance: Some(ToolProvenance::new(kind, "builtin")),
            mutation: ToolMutationClass::Unknown,
        }
    }

    /// A tool of the TUI's own callback surface `source`
    /// ([`FILE_TOOLS_SOURCE`] or [`NOTES_SOURCE`]), with the mutation class
    /// the TUI declares for `name`.
    #[must_use]
    pub fn tui(source: &str, name: &str) -> Self {
        Self {
            provenance: Some(crate::tools::provenance(source)),
            mutation: tui_mutation(name),
        }
    }

    /// A tool of an MCP server.
    #[must_use]
    pub fn mcp(server: &str) -> Self {
        Self {
            provenance: Some(ToolProvenance::new(ToolSourceKind::Mcp, server)),
            mutation: ToolMutationClass::Unknown,
        }
    }

    /// The family this origin puts its tool in.
    #[must_use]
    pub fn family(&self) -> ToolFamily {
        let Some(provenance) = &self.provenance else {
            return ToolFamily::Foreign;
        };
        match provenance.kind {
            ToolSourceKind::Builtin
            | ToolSourceKind::Shell
            | ToolSourceKind::Comms
            | ToolSourceKind::Memory
            | ToolSourceKind::Schedule
            | ToolSourceKind::WorkGraph
            | ToolSourceKind::Mob => ToolFamily::Native,
            ToolSourceKind::Callback if provenance.source_id.as_str() == FILE_TOOLS_SOURCE => {
                ToolFamily::Files
            }
            ToolSourceKind::Callback if provenance.source_id.as_str() == NOTES_SOURCE => {
                ToolFamily::Notes
            }
            ToolSourceKind::Callback if provenance.source_id.as_str() == FORK_SOURCE => {
                ToolFamily::Fork
            }
            ToolSourceKind::Callback | ToolSourceKind::Mcp | ToolSourceKind::RustBundle => {
                ToolFamily::Foreign
            }
        }
    }
}

/// The mutation class the TUI declares for its own tools.
fn tui_mutation(name: &str) -> ToolMutationClass {
    match name {
        READ_FILE | GLOB | GREP | NOTE_LIST | NOTE_READ => ToolMutationClass::ReadOnly,
        WRITE_FILE | EDIT_FILE | NOTE_SAVE | NOTE_FORGET => ToolMutationClass::Mutating,
        _ => ToolMutationClass::Unknown,
    }
}

/// A session's tools by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCatalog {
    tools: HashMap<String, ToolOrigin>,
}

impl ToolCatalog {
    /// The catalog of `defs`. The TUI's own tools get the mutation class
    /// the TUI declares for them; every other tool `Unknown`.
    pub fn from_defs<'a>(defs: impl IntoIterator<Item = &'a ToolDef>) -> Self {
        let tools = defs
            .into_iter()
            .map(|def| {
                let name = def.name.to_string();
                let mut origin = ToolOrigin {
                    provenance: def.provenance.clone(),
                    mutation: ToolMutationClass::Unknown,
                };
                if matches!(origin.family(), ToolFamily::Files | ToolFamily::Notes) {
                    origin.mutation = tui_mutation(&name);
                }
                (name, origin)
            })
            .collect();
        Self { tools }
    }

    /// Add or replace one tool.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, origin: ToolOrigin) -> Self {
        self.tools.insert(name.into(), origin);
        self
    }

    /// The origin of `name`, if the session offers it.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ToolOrigin> {
        self.tools.get(name)
    }

    /// Number of tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the catalog is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// Source of a session's live tool definitions.
#[async_trait]
pub trait ToolOrigins: Send + Sync {
    /// The tools `session_id` currently offers. `None` when they cannot be
    /// read: every call is then judged as a [`ToolFamily::Foreign`] tool.
    async fn catalog(&self, session_id: &SessionId) -> Option<ToolCatalog>;
}

/// [`ToolOrigins`] over the TUI's runtime-backed service: the visible tool
/// definitions the session's task last published (never waits behind a
/// running turn).
pub struct ServiceToolOrigins {
    service: Weak<crate::bootstrap::TuiService>,
}

impl std::fmt::Debug for ServiceToolOrigins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceToolOrigins").finish_non_exhaustive()
    }
}

impl ServiceToolOrigins {
    /// Origins read from `service` (held weakly: the gate never keeps the
    /// service alive).
    #[must_use]
    pub fn new(service: &Arc<crate::bootstrap::TuiService>) -> Self {
        Self {
            service: Arc::downgrade(service),
        }
    }
}

#[async_trait]
impl ToolOrigins for ServiceToolOrigins {
    async fn catalog(&self, session_id: &SessionId) -> Option<ToolCatalog> {
        let service = self.service.upgrade()?;
        match service.published_live_visible_tool_defs(session_id).await {
            Ok(defs) => Some(ToolCatalog::from_defs(&defs)),
            Err(err) => {
                tracing::debug!(error = %err, session = %session_id, "tool definitions unavailable");
                None
            }
        }
    }
}

/// [`ToolOrigins`] that always answers with one fixed catalog (tests and
/// embedders whose tool set is static).
#[derive(Debug, Clone, Default)]
pub struct StaticToolOrigins(pub ToolCatalog);

#[async_trait]
impl ToolOrigins for StaticToolOrigins {
    async fn catalog(&self, _session_id: &SessionId) -> Option<ToolCatalog> {
        Some(self.0.clone())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn families_follow_provenance_not_names() {
        let defs = [
            ToolDef::new("shell", "", json!({}))
                .with_provenance(ToolProvenance::new(ToolSourceKind::Builtin, "builtin")),
            ToolDef::new(READ_FILE, "", json!({}))
                .with_provenance(crate::tools::provenance(FILE_TOOLS_SOURCE)),
            ToolDef::new(NOTE_SAVE, "", json!({}))
                .with_provenance(crate::tools::provenance(NOTES_SOURCE)),
            ToolDef::new(crate::fork::FORK, "", json!({}))
                .with_provenance(crate::tools::provenance(FORK_SOURCE)),
            // An MCP server offering a tool named like a builtin.
            ToolDef::new("task_list", "", json!({}))
                .with_provenance(ToolProvenance::new(ToolSourceKind::Mcp, "evil")),
            // A foreign callback surface claiming a TUI tool name.
            ToolDef::new(WRITE_FILE, "", json!({}))
                .with_provenance(crate::tools::provenance("someone-else")),
            ToolDef::new("glob", "", json!({})),
        ];
        let catalog = ToolCatalog::from_defs(&defs);
        let family = |n: &str| catalog.get(n).map(ToolOrigin::family);
        assert_eq!(family("shell"), Some(ToolFamily::Native));
        assert_eq!(family(READ_FILE), Some(ToolFamily::Files));
        assert_eq!(family(NOTE_SAVE), Some(ToolFamily::Notes));
        assert_eq!(family(crate::fork::FORK), Some(ToolFamily::Fork));
        assert_eq!(family("task_list"), Some(ToolFamily::Foreign));
        assert_eq!(family(WRITE_FILE), Some(ToolFamily::Foreign));
        assert_eq!(family("glob"), Some(ToolFamily::Foreign));
        assert_eq!(family("absent"), None);
        assert_eq!(
            catalog.get(READ_FILE).map(|o| o.mutation),
            Some(ToolMutationClass::ReadOnly)
        );
        assert_eq!(
            catalog.get(NOTE_SAVE).map(|o| o.mutation),
            Some(ToolMutationClass::Mutating)
        );
    }
}
