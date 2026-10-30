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

/// The built-in tool families and tool names a build marks grant-gated.
///
/// A gated family is mounted as if enabled; its names, like the gated tool
/// names, stay closed until a live grant opens them. Set per build (session
/// options or a mob profile) and persisted with the session's tooling, so a
/// resume restores it.
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
    /// attribution of the operator, when it has one.
    Operator {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller: Option<String>,
    },
    /// An approved grant request.
    Approval { approval_id: String },
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

/// A session's scoped tool grants, as the agent's boundary and dispatch gate
/// read them.
///
/// The runtime implements this over its grant store for the session's
/// subjects. Every answer comes from the stored rows and the host clock read
/// for that answer: nothing caches whether a grant is live.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionToolGrants: Send + Sync {
    /// The names of `gated` that a live grant opens at this moment.
    async fn open_names(&self, gated: &GatedToolTable) -> Result<ToolNameSet, ToolGrantRefusal>;

    /// One call's admission. The clock is read after every await inside, so
    /// the caller hands the call on with no await in between.
    async fn admit_call(
        &self,
        gated: &GatedToolTable,
        call: ToolGrantCall<'_>,
    ) -> Result<(), ToolGrantRefusal>;
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

    /// The gated names a live grant opens at this moment.
    ///
    /// # Errors
    /// The seam's refusal when the grants cannot be read.
    pub async fn open_names(&self) -> Result<ToolNameSet, ToolGrantRefusal> {
        self.grants.open_names(&self.table).await
    }

    /// One call's admission.
    ///
    /// # Errors
    /// The refusal that closes the call.
    pub async fn admit_call(&self, call: ToolGrantCall<'_>) -> Result<(), ToolGrantRefusal> {
        self.grants.admit_call(&self.table, call).await
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
