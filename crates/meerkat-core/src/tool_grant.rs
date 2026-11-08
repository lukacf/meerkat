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
//! (issuance, expiry at a supplied clock, revocation, the end of the run a
//! run-scoped grant was issued for, use admission). The runtime implements
//! [`SessionToolGrants`] over its store and hands it to the factory on the
//! session's runtime bindings. Core never reads a grant's phase: it asks the
//! seam for the open names at each model-request boundary of a run, for one
//! call's admission immediately before the dispatcher hands the call on, and
//! tells it when a run ends.
//!
//! A gated build also mounts [`REQUEST_TOOL_GRANT_TOOL`]
//! ([`ToolGrantRequestDispatcher`]): the agent asks its session's host for a
//! grant a person approves ([`SessionToolGrants::request_grant`]), and the
//! host holds a durable decision for it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::{SessionId, ToolName, ToolNameSet};

mod request_tool;

pub use request_tool::ToolGrantRequestDispatcher;

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

/// The built-in tool families and tool names a build marks grant-gated.
///
/// A gated family is mounted as if enabled; its names, like the gated tool
/// names, stay closed until a live grant opens them. Set per build (session
/// options or a mob profile) and persisted with the session's tooling, so a
/// resume restores it.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantGatedTools {
    /// Built-in tool family names (`shell`, `workgraph`, ...).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub families: BTreeSet<String>,
    /// Individual tool names, of any source.
    #[serde(default, skip_serializing_if = "ToolNameSet::is_empty")]
    pub tools: ToolNameSet,
}

impl GrantGatedTools {
    /// Every key a gate declares, exactly as the serde derive reads them. A
    /// definition parser compares an authored gate's keys against it, so a
    /// typo is reported instead of silently leaving the gate narrower.
    pub const FIELD_NAMES: &'static [&'static str] = &["families", "tools"];

    /// Whether nothing is gated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.families.is_empty() && self.tools.is_empty()
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

    /// Gate everything `other` gates too.
    pub fn extend(&mut self, other: &Self) {
        self.families.extend(other.families.iter().cloned());
        for name in other.tools.iter() {
            self.tools.insert(name.clone());
        }
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
        rest
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

/// One scoped tool grant's id: a UUID v4 the grant service mints, or the
/// UUID v5 an approved request derives from its decision
/// ([`ToolGrantId::approved`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ToolGrantId(uuid::Uuid);

/// The namespace of the grant ids approved requests derive
/// ([`ToolGrantId::approved`]).
const APPROVED_GRANT_ID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6f1c_2b8e_4d3a_4f5b_9c27_8e1d_0a6b_3c59);

impl ToolGrantId {
    /// A fresh id.
    #[must_use]
    pub fn mint() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// The id of the grant an approved request issues: a UUID v5 over the
    /// approving decision's id. A request knows its grant's id before it is
    /// decided, and a replayed execution meets the grant it issued.
    #[must_use]
    pub fn approved(decision_id: &crate::approval::decision::DecisionId) -> Self {
        Self(uuid::Uuid::new_v5(
            &APPROVED_GRANT_ID_NAMESPACE,
            decision_id.as_str().as_bytes(),
        ))
    }

    /// Parse a grant id: a version 4 UUID the grant service minted, or a
    /// version 5 UUID an approved request derived.
    ///
    /// # Errors
    /// [`ToolGrantIdError`] when `value` is neither.
    pub fn parse(value: &str) -> Result<Self, ToolGrantIdError> {
        let uuid = uuid::Uuid::parse_str(value).map_err(|_| ToolGrantIdError {
            value: value.to_string(),
        })?;
        if !matches!(uuid.get_version_num(), 4 | 5) {
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

/// A string that is not a tool grant id: neither a version 4 nor a version 5
/// UUID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("tool grant id {value:?} is not a version 4 or version 5 UUID")]
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
}

impl ToolGrantTarget {
    /// Whether the grant's target covers `name`.
    #[must_use]
    pub fn covers(&self, name: &str) -> bool {
        match self {
            Self::Tool { name: target } => target.as_str() == name,
            Self::Family { names, .. } => names.contains(name),
        }
    }

    /// The names the target covers.
    pub fn names(&self) -> impl Iterator<Item = &ToolName> {
        let (one, many) = match self {
            Self::Tool { name } => (Some(name), None),
            Self::Family { names, .. } => (None, Some(names)),
        };
        one.into_iter().chain(many.into_iter().flatten())
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
    /// A `current_run` scope whose subject runs no turn: an idle session, or
    /// an identity whose member runs no turn on this host. Nothing was
    /// issued or held.
    #[error("the subject runs no turn: a current_run scope needs a run")]
    RunNotActive,
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
    /// The provider's call id, for the refusal audit row.
    pub call_id: &'a str,
    /// The run the call belongs to, when it belongs to one.
    pub run_id: Option<&'a crate::RunId>,
}

/// The name of the tool an agent asks for a grant with: every grant-gated
/// build mounts it ([`ToolGrantRequestDispatcher`]).
pub const REQUEST_TOOL_GRANT_TOOL: &str = "request_tool_grant";

/// One grant request an agent asks for through [`REQUEST_TOOL_GRANT_TOOL`]:
/// a target its build gates, how long the grant lasts from its issuance,
/// why, and the run and tool call that ask (a replay of the call answers the
/// same request). It names neither its subject (the agent's own session) nor
/// its approver (the host's policy).
#[derive(Debug, Clone, Copy)]
pub struct ToolGrantAgentRequest<'a> {
    pub target: &'a ToolGrantTarget,
    pub duration: std::time::Duration,
    pub reason: Option<&'a str>,
    pub run_id: &'a crate::RunId,
    pub call_id: &'a str,
}

/// An agent's grant request as its host holds it: the decision a person
/// approves, the id the grant gets when it is issued, the decision's phase
/// now and its deadline, how long the grant lasts once issued, and whether
/// the call joined a request already open for its target. A replayed call
/// answers the request it held, as it stands now; a call that joined a
/// request answers that request while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGrantRequestReceipt {
    pub decision_id: crate::approval::decision::DecisionId,
    pub grant_id: ToolGrantId,
    pub phase: crate::approval::decision::DecisionPhase,
    pub deadline_at_ms: u64,
    /// How long the grant lasts from its issuance, in milliseconds: the
    /// duration the request holds, which for a joined request is that
    /// request's and can differ from the one the call asked.
    pub duration_ms: u64,
    /// Whether the call joined a request already open or approved for its
    /// subject and target instead of holding its own.
    pub joined: bool,
}

/// Why an agent's grant request was not held.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ToolGrantAgentRequestRefusal {
    /// The session's host takes no grant requests.
    #[error("this host takes no tool grant requests")]
    NotTaken,
    /// The host refuses the request as asked: a duration outside its bounds,
    /// or a request it cannot hold.
    #[error("{0}")]
    Invalid(String),
    /// The request could not be held now.
    #[error("the tool grant request could not be held now: {0}")]
    Unavailable(String),
}

/// A session's scoped tool grants, as the agent's boundary and dispatch gate
/// read them.
///
/// The runtime implements this over its grant store for the session's
/// subjects. Every answer comes from the stored rows and the host clock read
/// for that answer: nothing caches whether a grant is live.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionToolGrants: Send + Sync {
    /// The names of `gated` that a live grant opens at this moment, in the
    /// run `run_id` (a model-request boundary belongs to one run). A grant
    /// scoped to another run opens nothing here, and is ended: the session's
    /// run moved on.
    async fn open_names(
        &self,
        gated: &GatedToolTable,
        run_id: Option<&crate::RunId>,
    ) -> Result<ToolNameSet, ToolGrantRefusal>;

    /// One call's admission. The clock is read after every await inside, so
    /// the caller hands the call on with no await in between.
    async fn admit_call(
        &self,
        gated: &GatedToolTable,
        call: ToolGrantCall<'_>,
    ) -> Result<(), ToolGrantRefusal>;

    /// The run `run_id` of the session ended (completed, failed or
    /// cancelled): end the grants scoped to it. Grants bounded by their
    /// window only, and grants of another run, are left as they are.
    ///
    /// # Errors
    /// `Unavailable` when the grants cannot be read or written. The ended
    /// run's grants then admit no use of a later run either: the first such
    /// use ends them.
    async fn end_run(&self, run_id: &crate::RunId) -> Result<(), ToolGrantRefusal>;

    /// Hold `request` for a person to approve: the host holds a durable
    /// decision for a grant of the target to this seam's session and issues
    /// nothing now. A replay of the same run and call answers the decision
    /// it held, as it stands.
    ///
    /// # Errors
    /// [`ToolGrantAgentRequestRefusal::NotTaken`] when the host takes no
    /// requests, as the default does; the host's refusal otherwise.
    async fn request_grant(
        &self,
        request: ToolGrantAgentRequest<'_>,
    ) -> Result<ToolGrantRequestReceipt, ToolGrantAgentRequestRefusal> {
        let _ = request;
        Err(ToolGrantAgentRequestRefusal::NotTaken)
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

/// A build's grant gate: the session's grants and what the build gated.
///
/// The factory installs one on the agent (the boundary's open set) and on the
/// outermost dispatch gate (each call's admission).
#[derive(Clone)]
pub struct ToolGrantGate {
    grants: Arc<dyn SessionToolGrants>,
    table: Arc<GatedToolTable>,
}

impl ToolGrantGate {
    /// The gate of `table` over `grants`.
    #[must_use]
    pub fn new(grants: Arc<dyn SessionToolGrants>, table: GatedToolTable) -> Self {
        Self {
            grants,
            table: Arc::new(table),
        }
    }

    /// What the build gated.
    #[must_use]
    pub fn table(&self) -> &GatedToolTable {
        &self.table
    }

    /// Whether `name` is gated.
    #[must_use]
    pub fn is_gated(&self, name: &str) -> bool {
        self.table.is_gated(name)
    }

    /// The gated names a live grant opens at this moment, in the run
    /// `run_id`.
    ///
    /// # Errors
    /// The seam's refusal when the grants cannot be read.
    pub async fn open_names(
        &self,
        run_id: Option<&crate::RunId>,
    ) -> Result<ToolNameSet, ToolGrantRefusal> {
        self.grants.open_names(&self.table, run_id).await
    }

    /// One call's admission.
    ///
    /// # Errors
    /// The refusal that closes the call.
    pub async fn admit_call(&self, call: ToolGrantCall<'_>) -> Result<(), ToolGrantRefusal> {
        self.grants.admit_call(&self.table, call).await
    }

    /// The run `run_id` ended: end the grants scoped to it.
    ///
    /// # Errors
    /// The seam's refusal when the grants cannot be read or written.
    pub async fn end_run(&self, run_id: &crate::RunId) -> Result<(), ToolGrantRefusal> {
        self.grants.end_run(run_id).await
    }

    /// The target an agent's request names, when the build gates it: one
    /// tool name the build gates (by name, or through a gated family), or a
    /// gated family with the names the build gated for it. `None` otherwise,
    /// and for a request naming both or neither.
    #[must_use]
    pub fn requested_target(
        &self,
        tool: Option<&str>,
        family: Option<&str>,
    ) -> Option<ToolGrantTarget> {
        match (tool, family) {
            (Some(name), None) if self.table.is_gated(name) => Some(ToolGrantTarget::Tool {
                name: ToolName::from(name),
            }),
            (None, Some(family)) => {
                self.table
                    .families()
                    .get(family)
                    .map(|names| ToolGrantTarget::Family {
                        family: family.to_owned(),
                        names: names.clone(),
                    })
            }
            _ => None,
        }
    }

    /// Ask the session's host to hold an agent's grant request
    /// ([`SessionToolGrants::request_grant`]).
    ///
    /// # Errors
    /// The host's refusal.
    pub async fn request_grant(
        &self,
        request: ToolGrantAgentRequest<'_>,
    ) -> Result<ToolGrantRequestReceipt, ToolGrantAgentRequestRefusal> {
        self.grants.request_grant(request).await
    }
}

impl std::fmt::Debug for ToolGrantGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolGrantGate")
            .field("table", &self.table)
            .finish_non_exhaustive()
    }
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
    fn an_approved_grant_id_derives_from_its_decision_and_parses() {
        use crate::approval::decision::{DecisionId, DecisionKind, DecisionSubjectKey};
        let decision = |subject: &str| {
            DecisionId::derive(
                &DecisionKind::new("tool_grant.issue").unwrap(),
                &DecisionSubjectKey::new(subject).unwrap(),
            )
        };
        let first = ToolGrantId::approved(&decision("s:one/tool:deploy/host:a"));
        assert_eq!(
            first,
            ToolGrantId::approved(&decision("s:one/tool:deploy/host:a")),
            "a replayed execution meets the id its decision derives"
        );
        assert_ne!(
            first,
            ToolGrantId::approved(&decision("s:one/tool:deploy/host:b")),
            "another decision derives another id"
        );
        assert_eq!(first.as_uuid().get_version_num(), 5);
        assert_eq!(ToolGrantId::parse(&first.to_string()).unwrap(), first);
        let text = serde_json::to_string(&first).unwrap();
        assert_eq!(serde_json::from_str::<ToolGrantId>(&text).unwrap(), first);
        assert_ne!(first, ToolGrantId::mint());
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
        assert!(target.covers("shell"));
        assert!(!target.covers("shell_job_cancel"));
        assert_eq!(target.names().count(), 2);
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
