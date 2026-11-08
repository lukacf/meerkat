//! Scoped tool grants: the core contracts.
//!
//! A build may mark built-in tool families and tool names *grant-gated*. A
//! gated name is mounted, but it stays hidden from the model and refused at
//! dispatch until a live scoped tool grant covering it is admitted. Grants
//! never widen past the build: they open only names the build gated, and
//! every other constraint (static policy, consequence policy, governed
//! authority, review, confinement) still applies on top.
//!
//! The canonical `ToolGrantLifecycleMachine` owns each grant's lifecycle
//! (issuance, expiry at a supplied clock, revocation, use admission). The
//! runtime implements [`SessionToolGrants`] over its store and hands it to
//! the factory on the session's runtime bindings. Core never reads a grant's
//! phase: it asks the seam for the open names at each model-request boundary,
//! and for one call's admission immediately before the dispatcher hands the
//! call on.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::{SessionId, ToolName, ToolNameSet};

/// Refusal code: no live grant covers the gated name.
pub const TOOL_GRANT_REQUIRED_CODE: &str = "tool_grant_required";
/// Refusal code: the grant covering the gated name reached its deadline.
pub const TOOL_GRANT_EXPIRED_CODE: &str = "tool_grant_expired";
/// Refusal code: the grant covering the gated name was revoked.
pub const TOOL_GRANT_REVOKED_CODE: &str = "tool_grant_revoked";
/// Refusal code: the grants could not be read, so the gated name stays closed.
pub const TOOL_GRANT_UNAVAILABLE_CODE: &str = "tool_grant_unavailable";

/// Why a build's scoped tool grant gate was refused: a gate no grant could
/// honour. The agent factory answers it, and a session service surfaces it as
/// [`crate::error::AgentError::ToolGrantGateRefused`], whose wire form is an
/// invalid-params error carrying this refusal, tagged by `kind`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum GrantGateRefusal {
    /// A gated family is not a built-in tool family.
    #[error("'{family}' is not a built-in tool family, so it cannot be grant-gated")]
    UnknownFamily { family: String },
    /// A gated family is disabled by this build; a gated family is mounted
    /// closed, never disabled.
    #[error("the tool family '{family}' is both disabled and grant-gated")]
    FamilyDisabled { family: String },
    /// A gated family none of whose names this build's final dispatcher
    /// mounts: no grant could ever open it.
    #[error("the tool family '{family}' is grant-gated, but this build mounts none of its tools")]
    FamilyNotMounted { family: String },
    /// A gated name a known tool source can provide, that this build's final
    /// dispatcher does not mount and no pending source could provide once it
    /// connects.
    #[error("'{tool}' is grant-gated, but this build does not mount it")]
    ToolNotMounted { tool: String },
    /// A gated tool name that no tool source of this build provides, and no
    /// pending source could provide once it connects.
    #[error("'{tool}' is grant-gated, but no tool source of this build provides it")]
    UnknownTool { tool: String },
    /// The build's tool policy refuses a gated name, so no grant could ever
    /// open it.
    #[error("'{tool}' is grant-gated, but the build's tool policy refuses it")]
    PolicyRefused { tool: String },
    /// The build has no grant service: a gate needs the runtime's, carried
    /// on the session's runtime bindings.
    #[error("a grant gate needs the runtime's tool grant service, and this build has none")]
    NoGrantService,
}

/// The built-in tool families, tool names and MCP servers a build marks
/// grant-gated.
///
/// A gated family is mounted as if enabled; its names, like the gated tool
/// names and every tool a gated MCP server provides, stay closed until a live
/// grant opens them. With `unconfigured_mcp_servers` the build also gates
/// every MCP server it does not configure ungated, at the build or attached
/// later. Set per build (session options or a mob profile) and persisted with
/// the session's tooling, so a resume restores it.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantGatedTools {
    /// Built-in tool family names (`shell`, `workgraph`, ...).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub families: BTreeSet<String>,
    /// Individual tool names, of any source.
    #[serde(default, skip_serializing_if = "ToolNameSet::is_empty")]
    pub tools: ToolNameSet,
    /// MCP server names: every tool such a server provides is gated, from the
    /// moment it appears. A gated server need not be configured: it may be
    /// attached later.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub mcp_servers: BTreeSet<String>,
    /// Gate every MCP server the build does not configure ungated: one an
    /// installation provides, at the build or later, and one attached later
    /// by any path.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unconfigured_mcp_servers: bool,
}

impl GrantGatedTools {
    /// Every key a gate declares, exactly as the serde derive reads them. A
    /// definition parser compares an authored gate's keys against it, so a
    /// typo is reported instead of silently leaving the gate narrower.
    pub const FIELD_NAMES: &'static [&'static str] = &[
        "families",
        "tools",
        "mcp_servers",
        "unconfigured_mcp_servers",
    ];

    /// Whether nothing is gated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
            && self.tools.is_empty()
            && self.mcp_servers.is_empty()
            && !self.unconfigured_mcp_servers
    }

    /// Gate a built-in tool family.
    #[must_use]
    pub fn with_family(mut self, family: impl Into<String>) -> Self {
        self.families.insert(family.into());
        self
    }

    /// Gate a tool name.
    #[must_use]
    pub fn with_tool(mut self, name: impl Into<ToolName>) -> Self {
        self.tools.insert(name);
        self
    }

    /// Gate an MCP server.
    #[must_use]
    pub fn with_mcp_server(mut self, server: impl Into<String>) -> Self {
        self.mcp_servers.insert(server.into());
        self
    }

    /// Gate every MCP server the build does not configure ungated.
    #[must_use]
    pub fn with_unconfigured_mcp_servers(mut self) -> Self {
        self.unconfigured_mcp_servers = true;
        self
    }

    /// Gate everything `other` gates too.
    pub fn extend(&mut self, other: &Self) {
        self.families.extend(other.families.iter().cloned());
        for name in other.tools.iter() {
            self.tools.insert(name.clone());
        }
        self.mcp_servers.extend(other.mcp_servers.iter().cloned());
        self.unconfigured_mcp_servers |= other.unconfigured_mcp_servers;
    }

    /// The entries of this gate that `other` does not gate.
    #[must_use]
    pub fn without(&self, other: &Self) -> Self {
        let mut rest = Self::default();
        rest.families.extend(
            self.families
                .iter()
                .filter(|family| !other.families.contains(*family))
                .cloned(),
        );
        for name in self.tools.iter() {
            if !other.tools.contains(name.as_str()) {
                rest.tools.insert(name.clone());
            }
        }
        rest.mcp_servers.extend(
            self.mcp_servers
                .iter()
                .filter(|server| !other.mcp_servers.contains(*server))
                .cloned(),
        );
        rest.unconfigured_mcp_servers =
            self.unconfigured_mcp_servers && !other.unconfigured_mcp_servers;
        rest
    }

    /// Whether this gate gates the MCP server `server` of a build that
    /// configures the servers `configured`: the gate names it, or the gate
    /// gates every server the build does not configure and `server` is not
    /// one of them.
    #[must_use]
    pub fn gates_mcp_server(&self, server: &str, configured: &BTreeSet<String>) -> bool {
        self.mcp_servers.contains(server)
            || (self.unconfigured_mcp_servers && !configured.contains(server))
    }
}

/// What one build gated, resolved: each gated family with the names it
/// provides, and the individually gated tool names.
///
/// A gated name may not be mounted yet (a tool of an MCP server that is still
/// connecting): it is gated by name from the moment it appears.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatedToolTable {
    families: BTreeMap<String, BTreeSet<ToolName>>,
    tools: BTreeSet<ToolName>,
}

impl GatedToolTable {
    /// The table of `families` (each with the names it provides) and `tools`.
    #[must_use]
    pub fn new(families: BTreeMap<String, BTreeSet<ToolName>>, tools: BTreeSet<ToolName>) -> Self {
        Self { families, tools }
    }

    /// Each gated family with the names it provides.
    #[must_use]
    pub fn families(&self) -> &BTreeMap<String, BTreeSet<ToolName>> {
        &self.families
    }

    /// The individually gated tool names.
    #[must_use]
    pub fn tools(&self) -> &BTreeSet<ToolName> {
        &self.tools
    }

    /// Every gated name: the families' names and the individual names.
    #[must_use]
    pub fn gated_names(&self) -> BTreeSet<ToolName> {
        self.families
            .values()
            .flatten()
            .chain(self.tools.iter())
            .cloned()
            .collect()
    }

    /// Whether `name` is gated.
    #[must_use]
    pub fn is_gated(&self, name: &str) -> bool {
        self.tools.contains(name) || self.families.values().any(|names| names.contains(name))
    }

    /// Whether nothing is gated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.families.values().all(BTreeSet::is_empty)
    }
}

/// A build's scoped tool grant gate as the generated visibility owner
/// installs it (`ReplaceToolGrantGate`): what the session's machine records
/// for the build until its next build.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledToolGrantGate {
    /// The tool names the build gated, by name or through a gated family.
    pub gated: BTreeSet<ToolName>,
    /// The MCP servers the build gated: every tool such a server provides is
    /// gated from the moment it appears.
    pub gated_servers: BTreeSet<String>,
    /// Whether the build gates every MCP server it does not configure
    /// ungated, at the build or attached later.
    pub gates_unconfigured_servers: bool,
    /// The servers the build configures ungated: while the mode above is on,
    /// the only servers it does not gate. Empty while the mode is off.
    pub ungated_servers: BTreeSet<String>,
    /// The role gate entries the build ignored as not mounted: never gated,
    /// never open.
    pub not_mounted_names: BTreeSet<ToolName>,
}

impl InstalledToolGrantGate {
    /// The gate of `gated` names alone.
    #[must_use]
    pub fn names(gated: BTreeSet<ToolName>) -> Self {
        Self {
            gated,
            ..Self::default()
        }
    }

    /// Whether the gate gates nothing: no name, no server and no mode. The
    /// ignored names alone gate nothing.
    #[must_use]
    pub fn gates_nothing(&self) -> bool {
        self.gated.is_empty() && self.gated_servers.is_empty() && !self.gates_unconfigured_servers
    }

    /// Whether the gate records nothing at all, its ignored names included.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.gates_nothing() && self.ungated_servers.is_empty() && self.not_mounted_names.is_empty()
    }
}

/// The scoped tool grant gate a live build runs, as its session's machine
/// records it: the gate the build installed, with the names a model-request
/// boundary since gated for a gated server, and the gated names a live grant
/// opened at the last boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunningToolGrantGate {
    /// What the build gates now.
    pub gate: InstalledToolGrantGate,
    /// The gated names a live grant opened at the last boundary.
    pub open: BTreeSet<ToolName>,
}

impl InstalledToolGrantGate {
    /// Whether the gate gates the MCP server `server`: a server it names, or,
    /// while it gates every server its build does not configure ungated, any
    /// server outside its ungated servers.
    #[must_use]
    pub fn gates_server(&self, server: &str) -> bool {
        self.gated_servers.contains(server)
            || (self.gates_unconfigured_servers && !self.ungated_servers.contains(server))
    }

    /// Whether the gate gates what `target` names: its tool name, any name
    /// of its family, or its MCP server.
    #[must_use]
    pub fn gates_target(&self, target: &ToolGrantTarget) -> bool {
        match target {
            ToolGrantTarget::McpServer { server, .. } => self.gates_server(server),
            ToolGrantTarget::Tool { .. } | ToolGrantTarget::Family { .. } => {
                target.names().any(|name| self.gated.contains(name))
            }
        }
    }

    /// Whether the build ignored what `target` names as not mounted: a name
    /// target all of whose names it ignored. A server is never ignored.
    #[must_use]
    pub fn ignores_target(&self, target: &ToolGrantTarget) -> bool {
        match target {
            ToolGrantTarget::McpServer { .. } => false,
            ToolGrantTarget::Tool { .. } | ToolGrantTarget::Family { .. } => {
                let mut names = target.names().peekable();
                names.peek().is_some() && names.all(|name| self.not_mounted_names.contains(name))
            }
        }
    }
}

/// One scoped tool grant's id: a UUID v4 the grant service mints.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ToolGrantId(uuid::Uuid);

impl ToolGrantId {
    /// A fresh id.
    #[must_use]
    pub fn mint() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Parse a UUID v4 id.
    ///
    /// # Errors
    /// [`ToolGrantIdError`] when `value` is not a version 4 UUID.
    pub fn parse(value: &str) -> Result<Self, ToolGrantIdError> {
        let uuid = uuid::Uuid::parse_str(value).map_err(|_| ToolGrantIdError {
            value: value.to_string(),
        })?;
        if uuid.get_version_num() != 4 {
            return Err(ToolGrantIdError {
                value: value.to_string(),
            });
        }
        Ok(Self(uuid))
    }

    /// The id's UUID.
    #[must_use]
    pub fn as_uuid(&self) -> &uuid::Uuid {
        &self.0
    }
}

impl std::fmt::Display for ToolGrantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<String> for ToolGrantId {
    type Error = ToolGrantIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ToolGrantId> for String {
    fn from(value: ToolGrantId) -> Self {
        value.0.to_string()
    }
}

/// A tool grant id that is not a version 4 UUID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("tool grant id {value:?} is not a version 4 UUID")]
pub struct ToolGrantIdError {
    pub value: String,
}

/// Who a grant applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolGrantSubject {
    /// One session, whatever builds serve it.
    Session { session_id: SessionId },
    /// One mob member identity within one mob incarnation, across the
    /// sessions that serve it.
    Identity {
        mob_id: String,
        mob_incarnation: String,
        identity: String,
    },
}

impl ToolGrantSubject {
    /// The subject of one session's grants.
    #[must_use]
    pub fn session(session_id: SessionId) -> Self {
        Self::Session { session_id }
    }

    /// The subject of one mob member identity's grants, in the incarnation
    /// of the mob that `incarnation` witnesses: the only constructor a member
    /// build or a grant surface uses for an identity. The incarnation is read
    /// from the mob runtime's sealed witness, never from decoded input, so an
    /// identity grant never reaches a later mob created under the same id.
    #[must_use]
    pub fn member(
        mob_id: impl Into<String>,
        incarnation: &crate::service::LiveMobIncarnation,
        identity: impl Into<String>,
    ) -> Self {
        Self::Identity {
            mob_id: mob_id.into(),
            mob_incarnation: incarnation.incarnation().to_string(),
            identity: identity.into(),
        }
    }

    /// The key a store files this subject's grants and audit rows under.
    #[must_use]
    pub fn key(&self) -> ToolGrantSubjectKey {
        match self {
            Self::Session { session_id } => ToolGrantSubjectKey(format!("s:{session_id}")),
            // Length-prefixed parts keep the key injective whatever the
            // parts contain.
            Self::Identity {
                mob_id,
                mob_incarnation,
                identity,
            } => ToolGrantSubjectKey(format!(
                "i:{}:{mob_id}{}:{mob_incarnation}{}:{identity}",
                mob_id.len(),
                mob_incarnation.len(),
                identity.len()
            )),
        }
    }
}

/// The store key of one subject's grants and audit rows: `s:<session>` for a
/// session, `i:` and the length-prefixed mob, incarnation and identity for an
/// identity. Data only: a key reads rows, it authorizes nothing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolGrantSubjectKey(String);

impl ToolGrantSubjectKey {
    /// The key's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A key read back from a store.
    #[must_use]
    pub fn from_stored(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl std::fmt::Display for ToolGrantSubjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a grant opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolGrantTarget {
    /// One tool name.
    Tool { name: ToolName },
    /// A built-in tool family, with the names it provided when the grant was
    /// issued: a tool the family gains later does not silently join it.
    Family {
        family: String,
        names: BTreeSet<ToolName>,
    },
    /// An MCP server: every tool whose provenance is the server at
    /// adjudication time, or, with `tools`, only those of its tools named
    /// there. No snapshot, unlike a family: the installed server is the
    /// surface. The names in `tools` only narrow: they open at most the named
    /// tools the server itself provides.
    McpServer {
        server: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools: Option<BTreeSet<ToolName>>,
    },
}

impl ToolGrantTarget {
    /// Whether the grant's target covers the tool `name`, provided by the MCP
    /// server `server` (`None` for a tool no MCP server provides).
    #[must_use]
    pub fn covers(&self, name: &str, server: Option<&str>) -> bool {
        match self {
            Self::Tool { name: target } => target.as_str() == name,
            Self::Family { names, .. } => names.contains(name),
            Self::McpServer {
                server: target,
                tools,
            } => {
                server == Some(target.as_str())
                    && tools.as_ref().is_none_or(|tools| tools.contains(name))
            }
        }
    }

    /// The names the target names: a tool's name, a family's names, a server
    /// target's `tools` subset (none for a whole server, which covers its
    /// tools by provenance).
    pub fn names(&self) -> impl Iterator<Item = &ToolName> {
        let (one, many) = match self {
            Self::Tool { name } => (Some(name), None),
            Self::Family { names, .. } => (None, Some(names)),
            Self::McpServer { tools, .. } => (None, tools.as_ref()),
        };
        one.into_iter().chain(many.into_iter().flatten())
    }

    /// The MCP server a server target covers.
    #[must_use]
    pub fn mcp_server(&self) -> Option<&str> {
        match self {
            Self::McpServer { server, .. } => Some(server.as_str()),
            Self::Tool { .. } | Self::Family { .. } => None,
        }
    }

    /// Whether the target names no tool: an empty tool name, or a server
    /// target whose `tools` subset is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Tool { name } => name.as_str().is_empty(),
            Self::Family { names, .. } => names.is_empty(),
            Self::McpServer { server, tools } => {
                server.is_empty() || tools.as_ref().is_some_and(BTreeSet::is_empty)
            }
        }
    }
}

/// Who issued a grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolGrantIssuer {
    /// An operator, through a trusted host surface. `caller` is the host's
    /// attribution of the operator, when it has one. `decision_ref` is the
    /// host's opaque reference to the decision it made before issuing (for
    /// example a person's approval in the host's own records): attribution
    /// only, written with the grant and never resolved, checked or trusted.
    Operator {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision_ref: Option<crate::approval::decision::DecisionReference>,
    },
    /// An approved grant request.
    Approval { approval_id: String },
}

impl ToolGrantIssuer {
    /// An operator issuer with no attribution and no decision reference.
    #[must_use]
    pub fn operator() -> Self {
        Self::Operator {
            caller: None,
            decision_ref: None,
        }
    }
}

/// One grant's immutable descriptor. The lifecycle machine never interprets
/// it; the grant service does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolGrantRecord {
    pub grant_id: ToolGrantId,
    pub subject: ToolGrantSubject,
    pub target: ToolGrantTarget,
    pub issuer: ToolGrantIssuer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Why a gated name is closed for a call.
///
/// When several grants cover one name, one admitted grant admits the call;
/// otherwise an expired grant explains the refusal, then a revoked one, and
/// with none of them the call needs a grant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolGrantRefusal {
    /// No live grant covers the name.
    #[error("no live tool grant covers the tool")]
    Required,
    /// The grant covering the name reached its deadline.
    #[error("tool grant {grant_id} expired at {expires_at_ms} ms")]
    Expired {
        grant_id: ToolGrantId,
        expires_at_ms: u64,
    },
    /// The grant covering the name was revoked.
    #[error("tool grant {grant_id} was revoked")]
    Revoked { grant_id: ToolGrantId },
    /// The grants could not be read or settled.
    #[error("tool grants are unavailable: {detail}")]
    Unavailable { detail: String },
}

impl ToolGrantRefusal {
    /// The refusal's stable code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Required => TOOL_GRANT_REQUIRED_CODE,
            Self::Expired { .. } => TOOL_GRANT_EXPIRED_CODE,
            Self::Revoked { .. } => TOOL_GRANT_REVOKED_CODE,
            Self::Unavailable { .. } => TOOL_GRANT_UNAVAILABLE_CODE,
        }
    }

    /// The model-facing denial for a call of `tool`. It names the tool and
    /// the deadline, never a store error.
    #[must_use]
    pub fn denial(&self, tool: &str) -> crate::ToolConsequenceDenial {
        let message = match self {
            Self::Required => format!("The tool '{tool}' needs a tool grant; none is active"),
            Self::Expired { expires_at_ms, .. } => format!(
                "The grant for '{tool}' expired at {}",
                format_unix_ms(*expires_at_ms)
            ),
            Self::Revoked { .. } => format!("The grant for '{tool}' was revoked"),
            Self::Unavailable { .. } => {
                format!("The tool grants for '{tool}' could not be checked; the tool stays closed")
            }
        };
        crate::ToolConsequenceDenial::new(self.code(), message)
    }

    /// The `policy_denied` tool error a call of `tool` is refused with.
    #[must_use]
    pub fn into_tool_error(self, tool: &str) -> crate::error::ToolError {
        crate::error::ToolError::policy_denied(self.denial(tool))
    }
}

/// Structured-data `kind` of a refused grant request on the wire. The
/// `tool_grant_request_refusal` member carries the typed
/// [`ToolGrantRequestRefusal`].
pub const TOOL_GRANT_REQUEST_REFUSED_KIND: &str = "tool_grant_request_refused";

/// Why a request to a grant surface (grant, revoke, list, audit, a host's
/// live attach) was refused: one closed vocabulary for every surface that
/// serves grants, answered as invalid params carrying the refusal under
/// [`TOOL_GRANT_REQUEST_REFUSED_KIND`]. A sibling of [`GrantGateRefusal`],
/// which refuses a build, not a request.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolGrantRequestRefusal {
    /// No such session or mob, or no such identity in the mob's roster.
    #[error("the grant subject is unknown")]
    SubjectUnknown,
    /// The subject's gate does not name the target: for an identity, the
    /// gate its next build resolves.
    #[error("the subject's grant gate does not name the target")]
    TargetNotGated,
    /// A server target that no installed or configured server provides: a
    /// host that keeps an installation list checks it.
    #[error("no installed or configured MCP server is named '{server}'")]
    ServerNotInstalled { server: String },
    /// A live attach of a server that no live grant of the identity covers,
    /// from a host that attaches granted servers live.
    #[error("no live grant covers the MCP server '{server}'")]
    NoLiveGrant { server: String },
    /// A live attach wait that is not a whole number of milliseconds within
    /// the host surface's bound.
    #[error("the attach wait is not a whole number of milliseconds within the bound")]
    InvalidWait,
    /// A window that is past, empty or longer than the maximum lifetime, or
    /// not bounded by exactly one of a duration and a deadline.
    #[error("the grant window is past, empty, too long, or not bounded exactly once")]
    InvalidWindow,
    /// A target that names no tool.
    #[error("a tool grant must name at least one tool")]
    EmptyTarget,
    /// An unknown or unparseable grant id.
    #[error("tool grant '{grant_id}' was not found")]
    NotFound { grant_id: String },
    /// A decision reference that is empty, too long or holds a control
    /// character.
    #[error("the decision reference is empty, too long or holds a control character")]
    InvalidDecisionRef,
    /// An audit page limit of zero.
    #[error("an audit page limit must be at least 1")]
    InvalidPageLimit,
}

impl ToolGrantRequestRefusal {
    /// The refusal's wire data: `{"kind": "tool_grant_request_refused",
    /// "tool_grant_request_refusal": {...}}`.
    #[must_use]
    pub fn data(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": TOOL_GRANT_REQUEST_REFUSED_KIND,
            "tool_grant_request_refusal": self,
        })
    }
}

/// RFC 3339 UTC rendering of UNIX milliseconds, or the raw value when it is
/// out of range.
fn format_unix_ms(unix_ms: u64) -> String {
    i64::try_from(unix_ms)
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map_or_else(
            || format!("{unix_ms} ms after the UNIX epoch"),
            |at| at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )
}

/// One call a grant gate adjudicates.
#[derive(Debug, Clone, Copy)]
pub struct ToolGrantCall<'a> {
    /// The called tool's name.
    pub name: &'a str,
    /// The MCP server that provides the called tool, from its provenance;
    /// `None` for a tool no MCP server provides.
    pub server: Option<&'a str>,
    /// The provider's call id, for the refusal audit row.
    pub call_id: &'a str,
    /// The run the call belongs to, when it belongs to one.
    pub run_id: Option<&'a crate::RunId>,
}

/// What a model-request boundary adjudicates of a build's gate: the gated
/// names (the session's machine's, after the boundary's extension) and the
/// names each gated MCP server provides now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatedToolView {
    gated: BTreeSet<ToolName>,
    server_names: BTreeMap<String, BTreeSet<ToolName>>,
}

impl GatedToolView {
    /// The view of the `gated` names, with each gated server's names now.
    #[must_use]
    pub fn new(
        gated: BTreeSet<ToolName>,
        server_names: BTreeMap<String, BTreeSet<ToolName>>,
    ) -> Self {
        Self {
            gated,
            server_names,
        }
    }

    /// The gated names.
    #[must_use]
    pub fn gated(&self) -> &BTreeSet<ToolName> {
        &self.gated
    }

    /// Whether `name` is gated.
    #[must_use]
    pub fn is_gated(&self, name: &str) -> bool {
        self.gated.contains(name)
    }

    /// The gated names `target` covers now: the gated names it names, and
    /// for a server target the gated names its server provides now, within
    /// its `tools` subset.
    #[must_use]
    pub fn covered_by(&self, target: &ToolGrantTarget) -> BTreeSet<ToolName> {
        match target {
            ToolGrantTarget::McpServer { server, tools } => self
                .server_names
                .get(server)
                .into_iter()
                .flatten()
                .filter(|name| {
                    self.gated.contains(*name)
                        && tools.as_ref().is_none_or(|tools| tools.contains(*name))
                })
                .cloned()
                .collect(),
            ToolGrantTarget::Tool { .. } | ToolGrantTarget::Family { .. } => target
                .names()
                .filter(|name| self.gated.contains(*name))
                .cloned()
                .collect(),
        }
    }
}

/// A session's scoped tool grants, as the agent's boundary, dispatch gate and
/// build read them.
///
/// The runtime implements this over its grant store for the session's
/// subjects. Every answer comes from the stored rows and the host clock read
/// for that answer: nothing caches whether a grant is live.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionToolGrants: Send + Sync {
    /// The gated names of `view` that a live grant opens at this moment.
    async fn open_names(&self, view: &GatedToolView) -> Result<ToolNameSet, ToolGrantRefusal>;

    /// One gated call's admission: a live grant that covers it admits it.
    /// The caller has already decided the call is gated. The clock is read
    /// after every await inside, so the caller hands the call on with no
    /// await in between.
    async fn admit_call(&self, call: ToolGrantCall<'_>) -> Result<(), ToolGrantRefusal>;

    /// Which of the MCP servers `servers` a live server grant covers at this
    /// moment, with or without a `tools` subset: a build attaches a gated
    /// server only while one does. A grant found past its deadline is
    /// settled as expired by the build. The default covers none.
    async fn granted_servers(
        &self,
        servers: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, ToolGrantRefusal> {
        let _ = servers;
        Ok(BTreeSet::new())
    }

    /// The seam of a mob member's build over the same grants: it reads the
    /// member's identity subject first (`member` in the mob incarnation that
    /// `incarnation` witnesses), then this seam's session, so a member's
    /// refusals are audited under its identity. `None` (the default) when this
    /// seam reads no identity subjects: the member build then reads its
    /// session's grants only.
    fn for_member(
        &self,
        member: &crate::MobMemberBinding,
        incarnation: &crate::service::LiveMobIncarnation,
    ) -> Option<Arc<dyn SessionToolGrants>> {
        let _ = (member, incarnation);
        None
    }
}

/// A build's grant gate: the session's grants, the gate the build installs,
/// and the build's visibility owner, whose machine records the gate the build
/// runs (the install, grown by each boundary's extension).
///
/// The factory installs one on the agent (the boundary's extension and open
/// set) and on the outermost dispatch gate (each call's admission).
#[derive(Clone)]
pub struct ToolGrantGate {
    grants: Arc<dyn SessionToolGrants>,
    install: Arc<InstalledToolGrantGate>,
    owner: crate::tool_scope::GeneratedToolVisibilityOwner,
}

impl ToolGrantGate {
    /// The gate `install` over `grants`, read through `owner`, the build's
    /// generated visibility owner.
    #[must_use]
    pub fn new(
        grants: Arc<dyn SessionToolGrants>,
        install: InstalledToolGrantGate,
        owner: crate::tool_scope::GeneratedToolVisibilityOwner,
    ) -> Self {
        Self {
            grants,
            install: Arc::new(install),
            owner,
        }
    }

    /// What the build installs.
    #[must_use]
    pub fn install(&self) -> &InstalledToolGrantGate {
        &self.install
    }

    /// Whether the tool `name`, provided by `provenance`, is gated by the
    /// gate the build runs now: its name is gated, or an MCP server the gate
    /// gates provides it. A gate that cannot be read gates every call, so a
    /// gated tool never runs ungated.
    #[must_use]
    pub fn is_gated(&self, name: &str, provenance: Option<&crate::types::ToolProvenance>) -> bool {
        match self.owner.visibility_state() {
            Ok(state) => state.tool_is_grant_gated(name, provenance),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    tool = name,
                    "the tool grant gate could not be read; the call is gated"
                );
                true
            }
        }
    }

    /// The gated names of `view` a live grant opens at this moment.
    ///
    /// # Errors
    /// The seam's refusal when the grants cannot be read.
    pub async fn open_names(&self, view: &GatedToolView) -> Result<ToolNameSet, ToolGrantRefusal> {
        self.grants.open_names(view).await
    }

    /// One gated call's admission.
    ///
    /// # Errors
    /// The refusal that closes the call.
    pub async fn admit_call(&self, call: ToolGrantCall<'_>) -> Result<(), ToolGrantRefusal> {
        self.grants.admit_call(call).await
    }

    /// The build's seam over the session's grants.
    #[must_use]
    pub fn grants(&self) -> &Arc<dyn SessionToolGrants> {
        &self.grants
    }
}

impl std::fmt::Debug for ToolGrantGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolGrantGate")
            .field("install", &self.install)
            .finish_non_exhaustive()
    }
}

/// A gate over `grants` that gates `names`, read through a fixed local
/// owner: the gate of a build whose machine recorded exactly those names.
#[cfg(test)]
pub(crate) fn gate_over_names(grants: Arc<dyn SessionToolGrants>, names: &[&str]) -> ToolGrantGate {
    let gated: BTreeSet<ToolName> = names.iter().map(|name| ToolName::from(*name)).collect();
    gate_over_install(grants, InstalledToolGrantGate::names(gated))
}

/// A gate over `grants` that runs `install`, read through a fixed local owner
/// whose machine recorded exactly that gate, with nothing open.
#[cfg(test)]
pub(crate) fn gate_over_install(
    grants: Arc<dyn SessionToolGrants>,
    install: InstalledToolGrantGate,
) -> ToolGrantGate {
    let state = crate::session::SessionToolVisibilityState {
        tool_grant_gated_names: install.gated.clone(),
        tool_grant_gated_servers: install.gated_servers.clone(),
        tool_grant_gates_unconfigured_servers: install.gates_unconfigured_servers,
        tool_grant_ungated_servers: install.ungated_servers.clone(),
        tool_grant_not_mounted_names: install.not_mounted_names.clone(),
        ..Default::default()
    };
    ToolGrantGate::new(
        grants,
        install,
        crate::tool_scope::GeneratedToolVisibilityOwner::from_generated_authority(Arc::new(
            crate::tool_scope::LocalToolVisibilityOwner::with_state(state),
        )),
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn subject_keys_are_injective() {
        let a = ToolGrantSubject::Identity {
            mob_id: "a:b".into(),
            mob_incarnation: "c".into(),
            identity: "d".into(),
        };
        let b = ToolGrantSubject::Identity {
            mob_id: "a".into(),
            mob_incarnation: "b:c".into(),
            identity: "d".into(),
        };
        assert_ne!(a.key(), b.key());
        let session = ToolGrantSubject::session(SessionId::new());
        assert!(session.key().as_str().starts_with("s:"));
    }

    #[test]
    fn refusals_map_to_policy_denied_with_their_codes() {
        let grant_id = ToolGrantId::mint();
        for (refusal, code) in [
            (ToolGrantRefusal::Required, TOOL_GRANT_REQUIRED_CODE),
            (
                ToolGrantRefusal::Expired {
                    grant_id: grant_id.clone(),
                    expires_at_ms: 0,
                },
                TOOL_GRANT_EXPIRED_CODE,
            ),
            (
                ToolGrantRefusal::Revoked { grant_id },
                TOOL_GRANT_REVOKED_CODE,
            ),
            (
                ToolGrantRefusal::Unavailable {
                    detail: "store offline".into(),
                },
                TOOL_GRANT_UNAVAILABLE_CODE,
            ),
        ] {
            let error = refusal.into_tool_error("shell");
            assert_eq!(error.error_code(), "policy_denied");
            match error {
                crate::error::ToolError::PolicyDenied { denial } => {
                    assert_eq!(denial.code, code);
                    assert!(denial.message.contains("'shell'"));
                    assert!(!denial.message.contains("store offline"));
                }
                other => panic!("expected policy_denied, got {other:?}"),
            }
        }
    }

    #[test]
    fn grant_ids_round_trip_and_refuse_non_v4_text() {
        let id = ToolGrantId::mint();
        let text = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<ToolGrantId>(&text).unwrap(), id);
        assert!(ToolGrantId::parse("not-a-uuid").is_err());
        assert!(ToolGrantId::parse("00000000-0000-1000-8000-000000000000").is_err());
    }

    #[test]
    fn a_family_target_covers_only_its_snapshot() {
        let target = ToolGrantTarget::Family {
            family: "shell".into(),
            names: ["shell", "shell_jobs"]
                .into_iter()
                .map(ToolName::from)
                .collect(),
        };
        assert!(target.covers("shell", None));
        assert!(!target.covers("shell_job_cancel", None));
        assert_eq!(target.names().count(), 2);
        assert_eq!(target.mcp_server(), None);
    }

    /// A server target covers the tools whose provenance is its server, or
    /// with `tools` only those of them it names; it names no tool of its own
    /// otherwise, and an empty server name or `tools` subset is an empty
    /// target. Its stored form is additive.
    #[test]
    fn a_server_target_covers_its_servers_tools_by_provenance() {
        let whole = ToolGrantTarget::McpServer {
            server: "tickets".into(),
            tools: None,
        };
        assert!(whole.covers("open_ticket", Some("tickets")));
        assert!(!whole.covers("open_ticket", Some("calendar")));
        assert!(
            !whole.covers("open_ticket", None),
            "a tool no server provides is never a server's"
        );
        assert_eq!(whole.names().count(), 0);
        assert_eq!(whole.mcp_server(), Some("tickets"));
        assert!(!whole.is_empty());

        let narrowed = ToolGrantTarget::McpServer {
            server: "tickets".into(),
            tools: Some(BTreeSet::from([ToolName::from("open_ticket")])),
        };
        assert!(narrowed.covers("open_ticket", Some("tickets")));
        assert!(!narrowed.covers("close_ticket", Some("tickets")));
        assert!(!narrowed.covers("open_ticket", Some("calendar")));
        assert_eq!(narrowed.names().count(), 1);
        assert_eq!(
            serde_json::to_value(&narrowed).unwrap(),
            serde_json::json!({"kind": "mcp_server", "server": "tickets", "tools": ["open_ticket"]})
        );
        assert_eq!(
            serde_json::to_value(&whole).unwrap(),
            serde_json::json!({"kind": "mcp_server", "server": "tickets"})
        );

        for empty in [
            ToolGrantTarget::McpServer {
                server: String::new(),
                tools: None,
            },
            ToolGrantTarget::McpServer {
                server: "tickets".into(),
                tools: Some(BTreeSet::new()),
            },
        ] {
            assert!(empty.is_empty(), "{empty:?}");
        }
    }

    /// A gate's servers and its unconfigured-servers mode count as gating
    /// something, join and subtract as its names do, and read in its key list.
    #[test]
    fn a_gate_counts_its_servers_and_its_mode() {
        assert!(GrantGatedTools::default().is_empty());
        let server = GrantGatedTools::default().with_mcp_server("tickets");
        let mode = GrantGatedTools::default().with_unconfigured_mcp_servers();
        assert!(!server.is_empty() && !mode.is_empty());

        let mut joined = server.clone();
        joined.extend(&mode);
        assert_eq!(
            joined,
            GrantGatedTools::default()
                .with_mcp_server("tickets")
                .with_unconfigured_mcp_servers()
        );
        assert_eq!(joined.without(&server), mode);
        assert_eq!(joined.without(&mode), server);
        assert!(joined.without(&joined).is_empty());

        let configured = BTreeSet::from(["calendar".to_string()]);
        assert!(server.gates_mcp_server("tickets", &configured));
        assert!(!server.gates_mcp_server("calendar", &configured));
        assert!(mode.gates_mcp_server("tickets", &configured));
        assert!(
            !mode.gates_mcp_server("calendar", &configured),
            "the mode leaves a server its build configures ungated"
        );

        let parsed: GrantGatedTools = serde_json::from_value(serde_json::json!({
            "mcp_servers": ["tickets"],
            "unconfigured_mcp_servers": true,
        }))
        .unwrap();
        assert_eq!(parsed, joined);
        assert_eq!(
            serde_json::to_value(GrantGatedTools::default().with_family("shell")).unwrap(),
            serde_json::json!({"families": ["shell"]}),
            "an ungated server list and an unset mode are omitted"
        );
        for key in ["mcp_servers", "unconfigured_mcp_servers"] {
            assert!(GrantGatedTools::FIELD_NAMES.contains(&key), "{key}");
        }
    }

    /// The installed gate's one server predicate: a server it names, or
    /// with the mode on any server outside its ungated servers. A target is
    /// gated when a name it names is gated or its server is gated, and
    /// ignored when every name it names was ignored as not mounted.
    #[test]
    fn the_installed_gate_judges_servers_and_targets() {
        let gate = InstalledToolGrantGate {
            gated: BTreeSet::from([ToolName::from("deploy")]),
            gated_servers: BTreeSet::from(["tickets".to_string()]),
            gates_unconfigured_servers: false,
            ungated_servers: BTreeSet::new(),
            not_mounted_names: BTreeSet::from([ToolName::from("shell")]),
        };
        assert!(gate.gates_server("tickets"));
        assert!(!gate.gates_server("calendar"));
        let moded = InstalledToolGrantGate {
            gates_unconfigured_servers: true,
            ungated_servers: BTreeSet::from(["calendar".to_string()]),
            ..gate.clone()
        };
        assert!(moded.gates_server("mail"));
        assert!(!moded.gates_server("calendar"));
        assert!(!moded.gates_nothing());

        let tool = |name: &str| ToolGrantTarget::Tool {
            name: ToolName::from(name),
        };
        let server = |server: &str| ToolGrantTarget::McpServer {
            server: server.into(),
            tools: None,
        };
        assert!(gate.gates_target(&tool("deploy")));
        assert!(!gate.gates_target(&tool("shell")));
        assert!(gate.ignores_target(&tool("shell")));
        assert!(!gate.ignores_target(&tool("deploy")));
        assert!(gate.gates_target(&server("tickets")));
        assert!(!gate.gates_target(&server("calendar")));
        assert!(
            !gate.ignores_target(&server("calendar")),
            "a server is never ignored"
        );

        let ignored = InstalledToolGrantGate {
            not_mounted_names: BTreeSet::from([ToolName::from("shell")]),
            ..InstalledToolGrantGate::default()
        };
        assert!(ignored.gates_nothing() && !ignored.is_empty());
        assert!(InstalledToolGrantGate::default().is_empty());
    }

    /// The view a boundary adjudicates: a name or family target opens the
    /// gated names it names; a server target opens the gated names its
    /// server provides now, within its `tools`, and nothing of another
    /// server or of a name that is not gated.
    #[test]
    fn a_boundary_view_covers_a_servers_gated_names_now() {
        let view = GatedToolView::new(
            ["deploy", "open_ticket", "close_ticket"]
                .into_iter()
                .map(ToolName::from)
                .collect(),
            BTreeMap::from([(
                "tickets".to_string(),
                ["open_ticket", "close_ticket", "not_yet_gated"]
                    .into_iter()
                    .map(ToolName::from)
                    .collect(),
            )]),
        );
        let names = |names: &[&str]| -> BTreeSet<ToolName> {
            names.iter().map(|name| ToolName::from(*name)).collect()
        };
        assert_eq!(
            view.covered_by(&ToolGrantTarget::McpServer {
                server: "tickets".into(),
                tools: None,
            }),
            names(&["close_ticket", "open_ticket"])
        );
        assert_eq!(
            view.covered_by(&ToolGrantTarget::McpServer {
                server: "tickets".into(),
                tools: Some(names(&["open_ticket", "elsewhere"])),
            }),
            names(&["open_ticket"])
        );
        assert!(
            view.covered_by(&ToolGrantTarget::McpServer {
                server: "calendar".into(),
                tools: None,
            })
            .is_empty()
        );
        assert_eq!(
            view.covered_by(&ToolGrantTarget::Tool {
                name: ToolName::from("deploy"),
            }),
            names(&["deploy"])
        );
        assert!(
            view.covered_by(&ToolGrantTarget::Tool {
                name: ToolName::from("read_file"),
            })
            .is_empty()
        );
        assert!(view.is_gated("open_ticket") && !view.is_gated("not_yet_gated"));
    }

    /// The one predicate every reader uses: a name the gate gates, or a tool
    /// whose MCP provenance names a server the gate gates (with the mode on,
    /// any MCP server outside the ungated ones); never a host's callback tool
    /// or a bundle, which are no MCP provenance.
    #[test]
    fn a_tool_is_gated_by_name_or_by_its_mcp_server() {
        use crate::types::{ToolProvenance, ToolSourceKind};
        let source = |kind, source: &str| ToolProvenance {
            kind,
            source_id: source.into(),
            artifact: None,
        };
        let state = crate::session::SessionToolVisibilityState {
            tool_grant_gated_names: BTreeSet::from([ToolName::from("deploy")]),
            tool_grant_gated_servers: BTreeSet::from(["tickets".to_string()]),
            tool_grant_gates_unconfigured_servers: true,
            tool_grant_ungated_servers: BTreeSet::from(["calendar".to_string()]),
            tool_grant_open_names: BTreeSet::from([ToolName::from("open_ticket")]),
            ..Default::default()
        };
        let tickets = source(ToolSourceKind::Mcp, "tickets");
        assert!(state.tool_is_grant_gated("deploy", None));
        assert!(state.tool_is_grant_gated("close_ticket", Some(&tickets)));
        assert!(
            state.tool_is_grant_gated("send_mail", Some(&source(ToolSourceKind::Mcp, "mail"))),
            "the mode gates a server the build does not configure"
        );
        assert!(
            !state.tool_is_grant_gated("today", Some(&source(ToolSourceKind::Mcp, "calendar")))
        );
        for kind in [ToolSourceKind::Callback, ToolSourceKind::RustBundle] {
            let named = format!("{kind:?}");
            assert!(
                !state.tool_is_grant_gated("host_tool", Some(&source(kind, "mail"))),
                "{named} provenance is never a server's"
            );
        }
        assert!(!state.tool_is_closed_by_grant_gate("open_ticket", Some(&tickets)));
        assert!(state.tool_is_closed_by_grant_gate("close_ticket", Some(&tickets)));
    }

    #[test]
    fn the_gated_table_names_every_family_name_and_tool() {
        let table = GatedToolTable::new(
            BTreeMap::from([(
                "shell".to_string(),
                ["shell", "shell_jobs"]
                    .into_iter()
                    .map(ToolName::from)
                    .collect(),
            )]),
            ["mcp_deploy"].into_iter().map(ToolName::from).collect(),
        );
        assert!(table.is_gated("shell_jobs"));
        assert!(table.is_gated("mcp_deploy"));
        assert!(!table.is_gated("read_file"));
        assert_eq!(table.gated_names().len(), 3);
    }
}
