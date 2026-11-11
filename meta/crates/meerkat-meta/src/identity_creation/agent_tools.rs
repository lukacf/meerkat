//! The agent tools of the identity family: what an agent calls to create and
//! change long-term agents.
//!
//! This is the surface's shape only: the seven tool names, who calls and
//! where a call ran, and what a tool answers. The tools are not built yet:
//! they carry no text, no member is offered them and every call is
//! answered as an unknown tool.

use std::sync::{Arc, LazyLock};

use meerkat_core::ToolCatalogEntry;
use meerkat_core::types::{SessionId, ToolDef, ToolProvenance, ToolSourceKind};
use meerkat_mob::MobHandle;
use serde_json::{Value, json};

use super::executor::ChangeRequester;
use super::family::IdentityFamily;

/// Create one or more long-term agents.
pub const IDENTITY_CREATE: &str = "identity_create";
/// Change one long-term agent.
pub const IDENTITY_UPDATE: &str = "identity_update";
/// Stop one long-term agent and keep it.
pub const IDENTITY_ARCHIVE: &str = "identity_archive";
/// Bring one archived long-term agent back.
pub const IDENTITY_UNARCHIVE: &str = "identity_unarchive";
/// Remove one long-term agent for good.
pub const IDENTITY_REMOVE: &str = "identity_remove";
/// List the long-term agents the caller's grant covers.
pub const IDENTITY_LIST: &str = "identity_list";
/// List what the caller may name in a create or an update.
pub const IDENTITY_DIRECTORY: &str = "identity_directory";

/// The identity tools, in the order a catalog lists them.
pub const IDENTITY_TOOLS: [&str; 7] = [
    IDENTITY_CREATE,
    IDENTITY_UPDATE,
    IDENTITY_ARCHIVE,
    IDENTITY_UNARCHIVE,
    IDENTITY_REMOVE,
    IDENTITY_LIST,
    IDENTITY_DIRECTORY,
];

/// Whether `name` is one of the identity tools.
pub fn is_identity_tool(name: &str) -> bool {
    IDENTITY_TOOLS.contains(&name)
}

/// Where the identity tools come from.
pub fn identity_tool_provenance() -> ToolProvenance {
    ToolProvenance {
        kind: ToolSourceKind::Mob,
        source_id: "mob".into(),
        artifact: None,
    }
}

static TOOL_DEFS: LazyLock<Arc<[Arc<ToolDef>]>> = LazyLock::new(|| {
    IDENTITY_TOOLS
        .iter()
        .map(|name| Arc::new(ToolDef::new(*name, "", json!({"type": "object"}))))
        .collect::<Vec<_>>()
        .into()
});

/// The definitions of the identity tools.
pub fn identity_tool_defs() -> Arc<[Arc<ToolDef>]> {
    Arc::clone(&TOOL_DEFS)
}

/// The identity tools as a catalog lists them.
pub fn identity_tool_catalog() -> Vec<ToolCatalogEntry> {
    identity_tool_defs()
        .iter()
        .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
        .collect()
}

/// The member whose agent calls an identity tool, as the dispatch that
/// carries the call attributes it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityToolCaller {
    /// The member's identity name.
    pub identity: String,
    /// The session the call ran in.
    pub session: SessionId,
    /// Whether the member is private to a person.
    pub private: bool,
}

impl IdentityToolCaller {
    /// The member `identity`, calling from `session`.
    pub fn new(identity: impl Into<String>, session: SessionId) -> Self {
        Self {
            identity: identity.into(),
            session,
            private: false,
        }
    }

    /// This caller, marked a member private to a person when `private`.
    #[must_use]
    pub fn private_to_a_person(mut self, private: bool) -> Self {
        self.private = private;
        self
    }

    /// The caller as the family takes a requester.
    pub fn requester(&self) -> ChangeRequester {
        ChangeRequester::agent_session(self.identity.clone(), self.session.clone())
            .private_to_a_person(self.private)
    }
}

/// Where one tool call ran within its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityToolCallSite<'a> {
    /// The run of the session the call belongs to, and the zero-based round
    /// of that run; `None` for a call dispatched outside a run's round.
    pub round: Option<(&'a str, u64)>,
    /// The call's id, as the model's response gave it.
    pub call_id: &'a str,
}

impl<'a> IdentityToolCallSite<'a> {
    /// The call `call_id` of round `round` of the run `run_id`.
    pub fn in_round(run_id: &'a str, round: u64, call_id: &'a str) -> Self {
        Self {
            round: Some((run_id, round)),
            call_id,
        }
    }

    /// The call `call_id`, dispatched outside any run's round.
    pub fn outside_a_run(call_id: &'a str) -> Self {
        Self {
            round: None,
            call_id,
        }
    }
}

/// What an identity tool answers: the result as the model reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityToolAnswer {
    /// The result: a change's typed outcome, or a read's answer.
    pub content: Value,
    /// Whether the call was refused: its result is then an error result.
    pub refused: bool,
}

impl IdentityToolAnswer {
    /// The result's text, as the tool result carries it.
    pub fn text(&self) -> String {
        self.content.to_string()
    }
}

/// Why an identity tool call has no answer of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityToolFault {
    /// The name is none of the identity tools.
    UnknownTool,
    /// The family answered a read with another read's answer.
    UnexpectedAnswer,
}

impl std::fmt::Display for IdentityToolFault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnknownTool => "not an identity tool",
            Self::UnexpectedAnswer => "the identity family answered another read",
        })
    }
}

impl std::error::Error for IdentityToolFault {}

/// The identity tools over one mob's family.
#[derive(Clone)]
pub struct IdentityAgentTools {
    #[expect(dead_code, reason = "the tools are not built yet")]
    family: Arc<dyn IdentityFamily>,
    mob: MobHandle,
}

impl IdentityAgentTools {
    /// The identity tools over `family`, in `mob`.
    pub fn new(family: Arc<dyn IdentityFamily>, mob: MobHandle) -> Self {
        Self { family, mob }
    }

    /// The mob the tools change.
    pub fn mob(&self) -> &MobHandle {
        &self.mob
    }

    /// One call of the identity tool `tool` by `caller`, with the call's
    /// `arguments` as the model wrote them, at `site`.
    pub async fn call(
        &self,
        _caller: &IdentityToolCaller,
        _site: &IdentityToolCallSite<'_>,
        _tool: &str,
        _arguments: &str,
    ) -> Result<IdentityToolAnswer, IdentityToolFault> {
        Err(IdentityToolFault::UnknownTool)
    }
}
