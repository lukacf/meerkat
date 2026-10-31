//! Meta-side sidecar table for mob- and run-level labels.
//!
//! Member-level labels are owned by `meerkat-mob` (they flow through
//! `SpawnMemberSpec.with_labels()` and out via `MobMemberListEntry.labels`).
//! Mob-level and run-level labels — for associating an external context like
//! `repo`, `branch`, `customer`, `deployment`, or `environment` with a mob or
//! a flow run — have nowhere to live in the upstream model. This module owns
//! that side table.
//!
//! For v1 the table is in-memory only. Persistence behind `MobStorage` is a
//! future enhancement; restarts wipe the labels. The table is keyed by
//! [`MetadataScope`] so the same surface can serve mobs and runs uniformly.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::Connection;
use serde_json::Value;
use tokio::sync::RwLock;

/// Scope of a label set.
///
/// Mob scope holds labels keyed by `mob_id`; run scope holds labels keyed by
/// `(mob_id, run_id)`. The mob id is part of the run scope so two mobs with
/// overlapping run identifiers stay isolated.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MetadataScope {
    Mob(String),
    Run(String, String),
}

impl MetadataScope {
    /// Return the mob id this scope belongs to.
    pub fn mob_id(&self) -> &str {
        match self {
            Self::Mob(mob) => mob,
            Self::Run(mob, _) => mob,
        }
    }

    /// Return the run id, if this scope is run-scoped.
    pub fn run_id(&self) -> Option<&str> {
        match self {
            Self::Mob(_) => None,
            Self::Run(_, run) => Some(run),
        }
    }
}

/// In-memory label table keyed by [`MetadataScope`].
///
/// Operations replace label sets wholesale (no merge). Callers wanting
/// merge semantics should read first, mutate the map, then write it back.
#[derive(Debug, Clone, Default)]
pub struct RuntimeMetadataTable {
    inner: Arc<RwLock<BTreeMap<MetadataScope, BTreeMap<String, String>>>>,
}

impl RuntimeMetadataTable {
    /// Create an empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the label set for `scope`. An empty `labels` map clears
    /// the entry.
    pub async fn set_labels(&self, scope: MetadataScope, labels: BTreeMap<String, String>) {
        let mut guard = self.inner.write().await;
        if labels.is_empty() {
            guard.remove(&scope);
        } else {
            guard.insert(scope, labels);
        }
    }

    /// Return the label set for `scope`, or an empty map if none is set.
    pub async fn get_labels(&self, scope: &MetadataScope) -> BTreeMap<String, String> {
        let guard = self.inner.read().await;
        guard.get(scope).cloned().unwrap_or_default()
    }

    /// Remove the label set for `scope`. Returns the previous value if any.
    pub async fn delete_labels(&self, scope: &MetadataScope) -> Option<BTreeMap<String, String>> {
        let mut guard = self.inner.write().await;
        guard.remove(scope)
    }

    /// Return all label sets associated with a mob — both the mob-scoped
    /// entry (if any) and every run-scoped entry whose mob id matches.
    pub async fn list_labels_for_mob(
        &self,
        mob_id: &str,
    ) -> Vec<(MetadataScope, BTreeMap<String, String>)> {
        let guard = self.inner.read().await;
        guard
            .iter()
            .filter(|(scope, _)| scope.mob_id() == mob_id)
            .map(|(scope, labels)| (scope.clone(), labels.clone()))
            .collect()
    }
}

/// Parse a JSON `labels` field as a string→string map.
///
/// Accepts a missing field, `null`, or an empty object — all yield an empty
/// map. Anything else must deserialize cleanly or returns a human-readable
/// error string suitable for a JSON-RPC `Invalid params` reply.
pub fn parse_labels_param(value: Option<&Value>) -> Result<BTreeMap<String, String>, String> {
    match value {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(v) => serde_json::from_value::<BTreeMap<String, String>>(v.clone())
            .map_err(|err| format!("labels must be a map of string to string: {err}")),
    }
}

/// Render a label map as a JSON object suitable for the wire format.
pub fn labels_to_json_value(labels: &BTreeMap<String, String>) -> Value {
    let mut map = serde_json::Map::with_capacity(labels.len());
    for (k, v) in labels {
        map.insert(k.clone(), Value::String(v.clone()));
    }
    Value::Object(map)
}

/// Outcome of dispatching a label RPC against a [`RuntimeMetadataTable`].
///
/// Both transports (the unified-runtime JSON-RPC and the HTTP-console JSON-RPC)
/// project this into their own response envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelRpcResult {
    /// `set` / `delete`: returns `{"accepted": true}`.
    Accepted,
    /// `get`: returns `{"labels": {...}}`.
    Labels(BTreeMap<String, String>),
    /// Validation error — `Invalid params: <message>`.
    InvalidParams(String),
}

/// Dispatch one mob/run label method through the transport-neutral domain.
///
/// HTTP and stdio deliberately keep their own access checks, notification
/// behavior, error-message decoration, and JSON envelope serialization. This
/// function is the single authority for the six methods' scope selection,
/// parameter validation, and metadata mutation. `None` means `method` does
/// not belong to the labels domain and must be handled by the caller's normal
/// method-not-found path.
pub async fn dispatch_label_method(
    table: &RuntimeMetadataTable,
    mob_id: &str,
    method: &str,
    params: &Value,
) -> Option<LabelRpcResult> {
    let scope = match method {
        "meta/mob_labels/set" | "meta/mob_labels/get" | "meta/mob_labels/delete" => {
            MetadataScope::Mob(mob_id.to_string())
        }
        "meta/run_labels/set" | "meta/run_labels/get" | "meta/run_labels/delete" => {
            match parse_run_id_param(params) {
                Ok(run_id) => MetadataScope::Run(mob_id.to_string(), run_id.to_string()),
                Err(message) => return Some(LabelRpcResult::InvalidParams(message)),
            }
        }
        _ => return None,
    };

    let outcome = match method {
        "meta/mob_labels/set" | "meta/run_labels/set" => {
            dispatch_labels_set(table, scope, params).await
        }
        "meta/mob_labels/get" | "meta/run_labels/get" => dispatch_labels_get(table, scope).await,
        "meta/mob_labels/delete" | "meta/run_labels/delete" => {
            dispatch_labels_delete(table, scope).await
        }
        _ => return None,
    };
    Some(outcome)
}

/// Replace the label set for `scope`, parsing `labels` from RPC params.
pub async fn dispatch_labels_set(
    table: &RuntimeMetadataTable,
    scope: MetadataScope,
    params: &Value,
) -> LabelRpcResult {
    match parse_labels_param(params.get("labels")) {
        Ok(labels) => {
            table.set_labels(scope, labels).await;
            LabelRpcResult::Accepted
        }
        Err(message) => LabelRpcResult::InvalidParams(message),
    }
}

/// Read the label set for `scope`.
pub async fn dispatch_labels_get(
    table: &RuntimeMetadataTable,
    scope: MetadataScope,
) -> LabelRpcResult {
    LabelRpcResult::Labels(table.get_labels(&scope).await)
}

/// Remove the label set for `scope`.
pub async fn dispatch_labels_delete(
    table: &RuntimeMetadataTable,
    scope: MetadataScope,
) -> LabelRpcResult {
    let _ = table.delete_labels(&scope).await;
    LabelRpcResult::Accepted
}

/// Pull a non-empty `run_id` string from RPC params.
pub fn parse_run_id_param(params: &Value) -> Result<&str, String> {
    match params.get("run_id").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err("run_id required".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Persistent metadata adapter
// ---------------------------------------------------------------------------
//
// Distinct from the in-memory `RuntimeMetadataTable` above. The label sidecar
// resets on restart (acceptable — labels are app-injected runtime metadata).
// The structural-events subscription cursor must survive restart so a
// restarted gateway resumes from where it left off rather than dropping
// events emitted between processes. This adapter owns that durable state.

/// Errors raised by [`PersistentMetadataStore`] implementations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataStoreError {
    /// Underlying I/O or storage failure (sqlite open, schema, query, ...).
    Io(String),
    /// A persisted value couldn't be parsed back into the typed shape — the
    /// store was probably written by a future meta version.
    Decode(String),
}

impl std::fmt::Display for MetadataStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "metadata store io: {msg}"),
            Self::Decode(msg) => write!(f, "metadata store decode: {msg}"),
        }
    }
}

impl std::error::Error for MetadataStoreError {}

/// One spawned member's durable idle-retirement opt-in.
///
/// A member seated in the primary mob is idle-retired only when its spawning
/// call opted it in (a `fork_off` child always is). The opt-in has to outlive
/// the process: after a restart the restored child is swept again only if its
/// opt-in is restored with it.
///
/// The opt-in belongs to one member instance, identified by the bridge
/// session it ran when the opt-in was recorded. A member id can be reused
/// after its member is retired; the sweep honours the record only while the
/// member seated under the id still runs `session_id`, and drops it
/// otherwise.
///
/// `recorded_at` orders the opt-in against mob events (a reset or destroy
/// releases only opt-ins recorded before it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberIdleRetireOverrideRecord {
    pub mob_id: String,
    pub member_id: String,
    pub session_id: meerkat_core::types::SessionId,
    pub policy: crate::mob_handle_runtime::DelegateIdleRetireOverride,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
}

/// Stored value of a member idle-retirement row (the key carries the member
/// id, the row's `mob_id` column the mob). Rows written before `recorded_at`
/// existed decode as recorded at the epoch.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredMemberIdleRetireOverride {
    session_id: meerkat_core::types::SessionId,
    policy: crate::mob_handle_runtime::DelegateIdleRetireOverride,
    #[serde(default)]
    recorded_at: chrono::DateTime<chrono::Utc>,
}

/// The one field a delete matches on, decoded from any version's row.
#[derive(serde::Deserialize)]
struct StoredMemberIdleRetireSession {
    session_id: meerkat_core::types::SessionId,
}

impl StoredMemberIdleRetireOverride {
    fn encode(record: &MemberIdleRetireOverrideRecord) -> Result<String, MetadataStoreError> {
        serde_json::to_string(&Self {
            session_id: record.session_id.clone(),
            policy: record.policy,
            recorded_at: record.recorded_at,
        })
        .map_err(|err| MetadataStoreError::Io(format!("encode idle-retire opt-in: {err}")))
    }
}

/// The visible tools of an identity's member as of its last build, which
/// `meta/identity/resolved_tools` answers for a member Core holds Dormant
/// without materializing it. Written from Core's report of each build, and
/// from the member's live tools after a warm or after the publication that
/// released a held build; a read never refreshes it. Kept in Meta's own record placements, which a
/// rollback to an earlier Meta ignores: rows of the `meta_metadata` table
/// (key `identity_tools/<identity>`) and, on a host store realm, a Meta
/// document (`tools/...`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IdentityToolRecord {
    pub mob_id: String,
    pub identity: String,
    /// The session the recorded build ran on.
    pub session_id: meerkat_core::types::SessionId,
    /// The built session's visible tool names, sorted.
    pub tools: Vec<String>,
    /// How many times the store recorded the identity's tools, this time
    /// included: it increases with every record written. It counts record
    /// writes only, and is not the identity's generation that status and
    /// inspect report. A record stored before the field's rename reads its
    /// `generation`.
    #[serde(alias = "generation")]
    pub record_revision: u64,
    /// When this record was written, in Unix milliseconds.
    pub published_at_ms: u64,
}

/// A fresh life (`reset`, `delete_identity`) Meta asked Core to start for an
/// identity, recorded before Core is asked and removed once Core answers.
/// A record that survives a restart names a rotation whose steps a crash
/// interrupted: the next roster compile drives it to its end. Kept in the
/// same placements as [`IdentityToolRecord`] (key
/// `identity_rotation/<identity>`; host store document `rotation/...`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IdentityRotationIntent {
    pub mob_id: String,
    pub identity: String,
    /// The verb that asked for the fresh life: `reset` or `delete_identity`.
    pub verb: String,
    /// The generation of Core's binding of the identity when the rotation
    /// was asked; `None` when Core held none.
    pub from_generation: Option<u64>,
    /// When the rotation was asked, in Unix milliseconds.
    pub recorded_at_ms: u64,
}

/// Where an identity runtime's lifecycle projection stands in its mob's
/// journal: every lifecycle side effect of the journal's events up to
/// `cursor` ran (the host's retirement hook, the distillation of a retired
/// session, the exit interview, `after_create`), except those of the events
/// `backlog` names, which a process stopped before their side effects ran
/// still owes. Written only after the side effects of the events it covers
/// ran, so a process that stops between the two runs them again at its next
/// start. Kept in the same placements as [`IdentityToolRecord`] (key
/// `lifecycle_projection/cursor`; host store document `lifecycle/...`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct LifecycleProjectionCursor {
    pub mob_id: String,
    /// The incarnation of the mob whose journal the cursor reads. A mob
    /// destroyed and created again under the same id starts a new journal,
    /// whose cursors start again, so a cursor of another incarnation names
    /// nothing in it.
    pub incarnation: String,
    /// The journal cursor of the last event whose side effects ran, apart
    /// from those `backlog` still owes.
    pub cursor: u64,
    /// The ranges of events before `cursor` whose side effects a stopped
    /// process still owed when this process started, in journal order: they
    /// run in the background, and each range goes once they ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backlog: Vec<LifecycleProjectionBacklog>,
    /// The intents of the retirements whose side effects ran, which may not
    /// be removed yet: the next start removes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncleared: Vec<LifecycleIntentKey>,
    /// When the cursor was written, in Unix milliseconds.
    pub recorded_at_ms: u64,
}

impl LifecycleProjectionCursor {
    /// A cursor at `cursor` of `mob_id`'s journal in the mob incarnation
    /// `incarnation`, with nothing owed before it.
    pub fn new(
        mob_id: impl Into<String>,
        incarnation: impl Into<String>,
        cursor: u64,
        recorded_at_ms: u64,
    ) -> Self {
        Self {
            mob_id: mob_id.into(),
            incarnation: incarnation.into(),
            cursor,
            backlog: Vec::new(),
            uncleared: Vec::new(),
            recorded_at_ms,
        }
    }
}

/// The events of a mob's journal after `after`, up to `until`, whose
/// lifecycle side effects a stopped process still owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct LifecycleProjectionBacklog {
    pub after: u64,
    pub until: u64,
}

impl LifecycleProjectionBacklog {
    pub fn new(after: u64, until: u64) -> Self {
        Self { after, until }
    }

    /// How many journal events the range still covers.
    pub fn events(&self) -> u64 {
        self.until.saturating_sub(self.after)
    }
}

/// The key of a lifecycle effect intent: the identity and the generation of
/// the incarnation its step ends.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct LifecycleIntentKey {
    pub identity: String,
    pub generation: u64,
}

impl LifecycleIntentKey {
    pub fn new(identity: impl Into<String>, generation: u64) -> Self {
        Self {
            identity: identity.into(),
            generation,
        }
    }
}

/// What one lifecycle step a Meta verb asks Core for owes beyond what Core's
/// journal records of it. Core journals that the step happened; this names
/// the verb that asked for it, since a reset and a delete are the same
/// rotation to Core, and the roster's unlisting the same retirement as any
/// other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LifecycleEffectVerb {
    /// The roster no longer lists the identity: Core retires its member.
    Unlisted,
    /// A reset: Core rotates the identity onto a fresh session.
    Reset,
    /// A delete: Core rotates the identity onto a fresh session, and the
    /// identity's memory continuity ends with the session it leaves.
    Delete,
    /// A role move: Core rotates the identity onto a fresh session under the
    /// role the roster now names, since its session has nothing to carry
    /// over (it lives in memory only, or no wake created it).
    RoleMove,
    /// A replacement on the identity's own session (a reload, a respawn, a
    /// compaction owner's rebuild): Core retires the member and the identity
    /// is seated again on the same session. It owes no side effect.
    Reload,
}

/// The side effects a Meta verb owes for one lifecycle step it asks Core
/// for, recorded before Core is asked. The identity runtime's lifecycle
/// projection reads it when it reaches the end of `generation` in Core's
/// journal, and removes it once that step's side effects ran. Kept in the
/// same placements as [`IdentityToolRecord`] (key
/// `lifecycle_intent/<generation>/<identity>`; host store document
/// `effect/...`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct LifecycleEffectIntent {
    pub mob_id: String,
    /// The incarnation of the mob the step runs in (see
    /// [`LifecycleProjectionCursor::incarnation`]).
    pub incarnation: String,
    pub identity: String,
    /// The generation of the incarnation the step ends.
    pub generation: u64,
    pub verb: LifecycleEffectVerb,
    /// The session of the incarnation the step ends, when it has one.
    pub session_id: Option<meerkat_core::types::SessionId>,
    /// When the step was asked, in Unix milliseconds. The exit interview the
    /// step queues is keyed by it, so a replay of the step queues it once.
    pub recorded_at_ms: u64,
    /// The cursor of the head of Core's member lifecycle journal, read
    /// before Core was asked for the step: a fact of the identity's member
    /// that Core journaled after it is the step's (or another step's of the
    /// same member), so a step that failed tells from the journal whether
    /// any of it reached Core. `None` for an intent recorded without it (by
    /// an earlier build, or when the head did not read): a step that failed
    /// then keeps its intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_after: Option<u64>,
    /// Core had retired the incarnation `generation` before the step was
    /// asked: the step (a reset, a delete) seats the identity's fresh life
    /// after it, and Core journals that seat as a fresh life after
    /// `generation`. The projection reads the intent at that seat, not at
    /// the incarnation's retirement, and runs the verb's memory side effects
    /// on `session_id`, with no retirement hook.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub predecessor_retired: bool,
    /// For a step whose incarnation Core had already retired
    /// (`predecessor_retired`): the intent of the step that ended that
    /// incarnation, which this one replaced before the projection reached
    /// the incarnation's retirement (an unlisting, say). The projection
    /// reads it at that retirement, and this intent at the fresh life's
    /// seat. `None` when it replaced none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<Box<LifecycleEffectIntent>>,
}

impl LifecycleEffectIntent {
    /// The intent of `verb` for `identity`'s incarnation `generation` (on
    /// `session_id`) in `mob_id`'s incarnation `incarnation`.
    pub fn new(
        mob_id: impl Into<String>,
        incarnation: impl Into<String>,
        identity: impl Into<String>,
        generation: u64,
        verb: LifecycleEffectVerb,
        session_id: Option<meerkat_core::types::SessionId>,
        recorded_at_ms: u64,
    ) -> Self {
        Self {
            mob_id: mob_id.into(),
            incarnation: incarnation.into(),
            identity: identity.into(),
            generation,
            verb,
            session_id,
            recorded_at_ms,
            asked_after: None,
            predecessor_retired: false,
            replaced: None,
        }
    }

    /// The intent, with the head of Core's journal its step read before
    /// Core was asked (`asked_after`).
    #[must_use]
    pub fn with_asked_after(mut self, asked_after: Option<u64>) -> Self {
        self.asked_after = asked_after;
        self
    }

    /// The intent, for a step whose incarnation Core had already retired
    /// before it was asked (`predecessor_retired`).
    #[must_use]
    pub fn with_predecessor_retired(mut self, predecessor_retired: bool) -> Self {
        self.predecessor_retired = predecessor_retired;
        self
    }

    /// The intent, with the intent of the step that ended its incarnation
    /// that it replaced before the projection reached that end
    /// (`replaced`).
    #[must_use]
    pub fn with_replaced(mut self, replaced: Option<LifecycleEffectIntent>) -> Self {
        self.replaced = replaced.map(Box::new);
        self
    }

    /// The intent's key.
    pub fn key(&self) -> LifecycleIntentKey {
        LifecycleIntentKey::new(self.identity.clone(), self.generation)
    }
}

/// A lifecycle side effect the identity runtime's projection could not run.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LifecycleEffectKind {
    /// The customizer's `after_create` for a seat.
    AfterCreate,
    /// The host's retirement hook for an incarnation Core ended.
    RetirementHook,
    /// The queueing of a retired identity's exit interview.
    ExitInterview,
    /// The distillation of a retired session.
    Distillation,
}

impl LifecycleEffectKind {
    /// The side effect's wire name: `after_create`, `retirement_hook`,
    /// `exit_interview` or `distillation`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AfterCreate => "after_create",
            Self::RetirementHook => "retirement_hook",
            Self::ExitInterview => "exit_interview",
            Self::Distillation => "distillation",
        }
    }
}

/// A lifecycle side effect Core's journal owed an identity that failed: its
/// every attempt missed its deadline or failed, or the host answered it with
/// an error. The projection recorded it before its cursor moved past the
/// event that owed it, and reported it on the error hook; the side effect
/// is not run again. Kept in the same placements as [`IdentityToolRecord`]
/// (key `lifecycle_failure/<generation>/<effect>/<identity>`; host store
/// document `failure/...`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct LifecycleEffectFailure {
    pub mob_id: String,
    /// The incarnation of the mob whose journal owed the side effect.
    pub incarnation: String,
    /// The cursor of the journal event that owed it.
    pub cursor: u64,
    pub identity: String,
    /// The generation of the incarnation it was owed for.
    pub generation: u64,
    pub effect: LifecycleEffectKind,
    /// The incarnation's session, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<meerkat_core::types::SessionId>,
    /// How many attempts were made.
    pub attempts: u32,
    /// What the last attempt failed with.
    pub error: String,
    /// When the failure was recorded, in Unix milliseconds.
    pub recorded_at_ms: u64,
}

impl LifecycleEffectFailure {
    /// The failure of `effect`, owed by the event at `cursor` of `mob_id`'s
    /// journal in the incarnation `incarnation` to `identity`'s incarnation
    /// `generation`, after `attempts` attempts, the last failing with
    /// `error`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mob_id: impl Into<String>,
        incarnation: impl Into<String>,
        cursor: u64,
        identity: impl Into<String>,
        generation: u64,
        effect: LifecycleEffectKind,
        attempts: u32,
        error: impl Into<String>,
        recorded_at_ms: u64,
    ) -> Self {
        Self {
            mob_id: mob_id.into(),
            incarnation: incarnation.into(),
            cursor,
            identity: identity.into(),
            generation,
            effect,
            session_id: None,
            attempts,
            error: error.into(),
            recorded_at_ms,
        }
    }

    /// The failure, naming the incarnation's session.
    #[must_use]
    pub fn with_session(mut self, session_id: Option<meerkat_core::types::SessionId>) -> Self {
        self.session_id = session_id;
        self
    }
}

/// The stored form of an identity record: the record and its format
/// version.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredIdentityRecord<T> {
    v: u32,
    #[serde(flatten)]
    record: T,
}

/// The format version of the identity records this build writes and reads.
const IDENTITY_RECORD_VERSION: u32 = 1;

/// Encode an identity record for the store.
pub(crate) fn encode_identity_record<T: serde::Serialize + Clone>(
    record: &T,
) -> Result<String, MetadataStoreError> {
    serde_json::to_string(&StoredIdentityRecord {
        v: IDENTITY_RECORD_VERSION,
        record: record.clone(),
    })
    .map_err(|err| MetadataStoreError::Io(format!("encode identity record: {err}")))
}

/// Decode an identity record; a record of another format version is
/// refused, never guessed.
pub(crate) fn decode_identity_record<T: serde::de::DeserializeOwned>(
    value: &[u8],
) -> Result<T, MetadataStoreError> {
    let stored: StoredIdentityRecord<T> = serde_json::from_slice(value)
        .map_err(|err| MetadataStoreError::Decode(format!("identity record: {err}")))?;
    if stored.v != IDENTITY_RECORD_VERSION {
        return Err(MetadataStoreError::Decode(format!(
            "identity record of format version {}",
            stored.v
        )));
    }
    Ok(stored.record)
}

/// Key of an identity's tool record in the `meta_metadata` table.
const IDENTITY_TOOLS_KEY_PREFIX: &str = "identity_tools/";

/// Key prefix of the identity rotation intents in the `meta_metadata` table.
const IDENTITY_ROTATION_KEY_PREFIX: &str = "identity_rotation/";

/// Key of a mob's lifecycle projection cursor in the `meta_metadata` table.
const LIFECYCLE_CURSOR_KEY: &str = "lifecycle_projection/cursor";

/// Key prefix of the lifecycle effect intents in the `meta_metadata` table.
const LIFECYCLE_INTENT_KEY_PREFIX: &str = "lifecycle_intent/";

/// Key prefix of the lifecycle effect failures in the `meta_metadata` table.
const LIFECYCLE_FAILURE_KEY_PREFIX: &str = "lifecycle_failure/";

/// The key of the lifecycle effect intent of `identity`'s `generation`: the
/// generation first, so the key is unambiguous whatever the identity holds.
fn lifecycle_intent_key(identity: &str, generation: u64) -> String {
    format!("{LIFECYCLE_INTENT_KEY_PREFIX}{generation}/{identity}")
}

/// The key of `failure`: its generation and effect first, both free of `/`,
/// so the key is unambiguous whatever the identity holds.
fn lifecycle_failure_key(failure: &LifecycleEffectFailure) -> String {
    format!(
        "{LIFECYCLE_FAILURE_KEY_PREFIX}{}/{}/{}",
        failure.generation,
        failure.effect.as_str(),
        failure.identity
    )
}

/// The order a mob's lifecycle effect failures read in: oldest first, then
/// by generation, effect and identity.
pub(crate) fn lifecycle_effect_failure_order(
    a: &LifecycleEffectFailure,
    b: &LifecycleEffectFailure,
) -> std::cmp::Ordering {
    (a.recorded_at_ms, a.generation, a.effect, &a.identity).cmp(&(
        b.recorded_at_ms,
        b.generation,
        b.effect,
        &b.identity,
    ))
}

/// The first key past every key that starts with `prefix`, a key prefix
/// ending with `/`: the same prefix ending with the byte after `/`, `0`. A
/// prefix read as the key range from `prefix` to this one keeps to the
/// `(mob_id, key)` index.
fn key_range_end(prefix: &str) -> String {
    format!("{}0", prefix.strip_suffix('/').unwrap_or(prefix))
}

/// Persistent storage for meta runtime metadata that must survive a
/// gateway restart: the structural-events subscription cursor and spawned
/// members' idle-retirement opt-ins.
///
/// Two impls live in this module: [`InMemoryMetadataStore`] (no
/// persistence; used when no SQLite mob storage is configured) and
/// [`SqliteMetadataStore`] (writes a small `meta_metadata` table next
/// to the mob's own SQLite store). The `UnifiedRuntime` builder picks
/// the impl based on the configured `MobBootstrapSpec`.
#[async_trait]
pub trait PersistentMetadataStore: Send + Sync {
    /// Read the last-projected mob events cursor for `mob_id`. Returns
    /// `Ok(None)` when no cursor has been written yet (fresh deploy or
    /// in-memory deployment that just started).
    async fn get_subscription_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<u64>, MetadataStoreError>;

    /// Persist the last-projected mob events cursor for `mob_id`.
    async fn set_subscription_cursor(
        &self,
        mob_id: &str,
        cursor: u64,
    ) -> Result<(), MetadataStoreError>;

    /// Every recorded member idle-retirement opt-in, across all mobs.
    ///
    /// The default records nothing: a store that predates this seam keeps
    /// opt-ins for the life of the process only, as before.
    async fn load_member_idle_retire_overrides(
        &self,
    ) -> Result<Vec<MemberIdleRetireOverrideRecord>, MetadataStoreError> {
        Ok(Vec::new())
    }

    /// Record (or replace) one member's idle-retirement opt-in.
    async fn set_member_idle_retire_override(
        &self,
        _record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        Ok(())
    }

    /// Forget this member instance's opt-in: the row is removed only while it
    /// is still bound to `record.session_id`, so an opt-in recorded for a
    /// newer member under the same id survives. Clearing an absent or
    /// different row is not an error.
    async fn clear_member_idle_retire_override(
        &self,
        _record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        Ok(())
    }

    /// `identity`'s last-published tool record in `mob_id`, if any.
    ///
    /// Required: a store that keeps no tool records would make every
    /// identity's resolved tools read `no_build_recorded` forever.
    async fn identity_tool_record(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Result<Option<IdentityToolRecord>, MetadataStoreError>;

    /// Record (or replace) an identity's last-published tool record. An
    /// error is the store's: the runtime logs it and keeps serving.
    async fn set_identity_tool_record(
        &self,
        record: &IdentityToolRecord,
    ) -> Result<(), MetadataStoreError>;

    /// Every rotation intent recorded for `mob_id`'s identities.
    ///
    /// Required: the records carry a rotation (a reset, a delete) a crash
    /// interrupted across the restart, so that the next roster compile
    /// drives it to its end.
    async fn load_identity_rotation_intents(
        &self,
        mob_id: &str,
    ) -> Result<Vec<IdentityRotationIntent>, MetadataStoreError>;

    /// Record (or replace) an identity's rotation intent, durably, before
    /// the rotation starts. An error refuses the rotation: nothing starts.
    async fn set_identity_rotation_intent(
        &self,
        intent: &IdentityRotationIntent,
    ) -> Result<(), MetadataStoreError>;

    /// Remove an identity's rotation intent; removing an absent one is not
    /// an error.
    async fn clear_identity_rotation_intent(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Result<(), MetadataStoreError>;

    /// `mob_id`'s lifecycle projection cursor, if one was written.
    ///
    /// Required: the cursor is what makes the identities' lifecycle side
    /// effects (the retirement hook, the distillation of a retired session,
    /// the exit interview, `after_create`) run at least once across a
    /// restart. A store that lost it would let the next start skip the
    /// effects a stopped process still owed.
    async fn lifecycle_projection_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<LifecycleProjectionCursor>, MetadataStoreError>;

    /// Record (or replace) `cursor.mob_id`'s lifecycle projection cursor,
    /// durably, before this answers.
    async fn set_lifecycle_projection_cursor(
        &self,
        cursor: &LifecycleProjectionCursor,
    ) -> Result<(), MetadataStoreError>;

    /// The lifecycle effect intent of `identity`'s `generation` in `mob_id`,
    /// if one was recorded.
    ///
    /// Required: the intent carries what a reset, a delete or the roster's
    /// unlisting owes across a restart that came between the step and its
    /// side effects.
    async fn lifecycle_effect_intent(
        &self,
        mob_id: &str,
        identity: &str,
        generation: u64,
    ) -> Result<Option<LifecycleEffectIntent>, MetadataStoreError>;

    /// Record (or replace) a lifecycle effect intent, durably, before the
    /// step it names is asked of Core. An error refuses the step: nothing
    /// starts.
    async fn set_lifecycle_effect_intent(
        &self,
        intent: &LifecycleEffectIntent,
    ) -> Result<(), MetadataStoreError>;

    /// Remove the lifecycle effect intent of `identity`'s `generation`;
    /// removing an absent one is not an error.
    async fn clear_lifecycle_effect_intent(
        &self,
        mob_id: &str,
        identity: &str,
        generation: u64,
    ) -> Result<(), MetadataStoreError>;

    /// Every lifecycle effect intent recorded for `mob_id`, in no promised
    /// order: what the start of a projection sweeps, and what a roster pass
    /// reads to take back an unlisting the roster undid.
    async fn lifecycle_effect_intents(
        &self,
        mob_id: &str,
    ) -> Result<Vec<LifecycleEffectIntent>, MetadataStoreError>;

    /// Record (or replace) the failure of one lifecycle side effect, durably,
    /// before the projection's cursor moves past the event that owed it.
    ///
    /// Required: a side effect the projection gives up on is recorded, so
    /// no failure is lost with the cursor that moved past it.
    async fn record_lifecycle_effect_failure(
        &self,
        failure: &LifecycleEffectFailure,
    ) -> Result<(), MetadataStoreError>;

    /// One page of the lifecycle effect failures recorded for `mob_id`,
    /// oldest first (then by generation, effect and identity): at most
    /// `limit` of them, after the first `skip`.
    async fn lifecycle_effect_failures(
        &self,
        mob_id: &str,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<LifecycleEffectFailure>, MetadataStoreError>;

    /// Keep the `keep` latest lifecycle effect failures recorded for
    /// `mob_id`, in the order [`Self::lifecycle_effect_failures`] reads them,
    /// and remove the older ones: how many it removed.
    ///
    /// Required: one failure is recorded per failed side effect, so a host
    /// hook that fails every retirement would otherwise leave one record per
    /// retirement for good. The projection's start prunes them.
    async fn prune_lifecycle_effect_failures(
        &self,
        mob_id: &str,
        keep: usize,
    ) -> Result<usize, MetadataStoreError>;

    /// Wait until the writes this store runs off its callers' tasks (such as
    /// coalesced cursor writes) have landed. Shutdown calls it once the last
    /// writer stopped, under its own bound. The default runs no write off
    /// its callers, so it returns at once.
    async fn settle_detached_writes(&self) {}
}

/// In-memory persistent metadata store.
///
/// "Persistent" is aspirational here — the values survive `Arc<...>` clones
/// but reset to empty on process restart. Used when no SQLite mob storage
/// is configured. The structural-events subscription falls back to "start
/// at latest" on restart in this case, which is the right behaviour:
/// in-memory deployments don't have a persistent ledger to replay against
/// either.
#[derive(Debug, Default)]
pub struct InMemoryMetadataStore {
    cursors: RwLock<BTreeMap<String, u64>>,
    idle_retire_overrides: RwLock<BTreeMap<(String, String), MemberIdleRetireOverrideRecord>>,
    identity_tool_records: RwLock<BTreeMap<(String, String), IdentityToolRecord>>,
    identity_rotation_intents: RwLock<BTreeMap<(String, String), IdentityRotationIntent>>,
    lifecycle_cursors: RwLock<BTreeMap<String, LifecycleProjectionCursor>>,
    lifecycle_intents: RwLock<BTreeMap<(String, String, u64), LifecycleEffectIntent>>,
    lifecycle_failures: RwLock<BTreeMap<(String, String), LifecycleEffectFailure>>,
}

impl InMemoryMetadataStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PersistentMetadataStore for InMemoryMetadataStore {
    async fn get_subscription_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<u64>, MetadataStoreError> {
        Ok(self.cursors.read().await.get(mob_id).copied())
    }

    async fn set_subscription_cursor(
        &self,
        mob_id: &str,
        cursor: u64,
    ) -> Result<(), MetadataStoreError> {
        self.cursors
            .write()
            .await
            .insert(mob_id.to_string(), cursor);
        Ok(())
    }

    async fn load_member_idle_retire_overrides(
        &self,
    ) -> Result<Vec<MemberIdleRetireOverrideRecord>, MetadataStoreError> {
        Ok(self
            .idle_retire_overrides
            .read()
            .await
            .values()
            .cloned()
            .collect())
    }

    async fn set_member_idle_retire_override(
        &self,
        record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        self.idle_retire_overrides.write().await.insert(
            (record.mob_id.clone(), record.member_id.clone()),
            record.clone(),
        );
        Ok(())
    }

    async fn clear_member_idle_retire_override(
        &self,
        record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        let mut overrides = self.idle_retire_overrides.write().await;
        let key = (record.mob_id.clone(), record.member_id.clone());
        if overrides
            .get(&key)
            .is_some_and(|stored| stored.session_id == record.session_id)
        {
            overrides.remove(&key);
        }
        Ok(())
    }

    async fn identity_tool_record(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Result<Option<IdentityToolRecord>, MetadataStoreError> {
        Ok(self
            .identity_tool_records
            .read()
            .await
            .get(&(mob_id.to_string(), identity.to_string()))
            .cloned())
    }

    async fn set_identity_tool_record(
        &self,
        record: &IdentityToolRecord,
    ) -> Result<(), MetadataStoreError> {
        self.identity_tool_records.write().await.insert(
            (record.mob_id.clone(), record.identity.clone()),
            record.clone(),
        );
        Ok(())
    }

    async fn load_identity_rotation_intents(
        &self,
        mob_id: &str,
    ) -> Result<Vec<IdentityRotationIntent>, MetadataStoreError> {
        Ok(self
            .identity_rotation_intents
            .read()
            .await
            .values()
            .filter(|intent| intent.mob_id == mob_id)
            .cloned()
            .collect())
    }

    async fn set_identity_rotation_intent(
        &self,
        intent: &IdentityRotationIntent,
    ) -> Result<(), MetadataStoreError> {
        self.identity_rotation_intents.write().await.insert(
            (intent.mob_id.clone(), intent.identity.clone()),
            intent.clone(),
        );
        Ok(())
    }

    async fn clear_identity_rotation_intent(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Result<(), MetadataStoreError> {
        self.identity_rotation_intents
            .write()
            .await
            .remove(&(mob_id.to_string(), identity.to_string()));
        Ok(())
    }

    async fn lifecycle_projection_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<LifecycleProjectionCursor>, MetadataStoreError> {
        Ok(self.lifecycle_cursors.read().await.get(mob_id).cloned())
    }

    async fn set_lifecycle_projection_cursor(
        &self,
        cursor: &LifecycleProjectionCursor,
    ) -> Result<(), MetadataStoreError> {
        self.lifecycle_cursors
            .write()
            .await
            .insert(cursor.mob_id.clone(), cursor.clone());
        Ok(())
    }

    async fn lifecycle_effect_intent(
        &self,
        mob_id: &str,
        identity: &str,
        generation: u64,
    ) -> Result<Option<LifecycleEffectIntent>, MetadataStoreError> {
        Ok(self
            .lifecycle_intents
            .read()
            .await
            .get(&(mob_id.to_string(), identity.to_string(), generation))
            .cloned())
    }

    async fn set_lifecycle_effect_intent(
        &self,
        intent: &LifecycleEffectIntent,
    ) -> Result<(), MetadataStoreError> {
        self.lifecycle_intents.write().await.insert(
            (
                intent.mob_id.clone(),
                intent.identity.clone(),
                intent.generation,
            ),
            intent.clone(),
        );
        Ok(())
    }

    async fn clear_lifecycle_effect_intent(
        &self,
        mob_id: &str,
        identity: &str,
        generation: u64,
    ) -> Result<(), MetadataStoreError> {
        self.lifecycle_intents.write().await.remove(&(
            mob_id.to_string(),
            identity.to_string(),
            generation,
        ));
        Ok(())
    }

    async fn lifecycle_effect_intents(
        &self,
        mob_id: &str,
    ) -> Result<Vec<LifecycleEffectIntent>, MetadataStoreError> {
        Ok(self
            .lifecycle_intents
            .read()
            .await
            .values()
            .filter(|intent| intent.mob_id == mob_id)
            .cloned()
            .collect())
    }

    async fn record_lifecycle_effect_failure(
        &self,
        failure: &LifecycleEffectFailure,
    ) -> Result<(), MetadataStoreError> {
        self.lifecycle_failures.write().await.insert(
            (failure.mob_id.clone(), lifecycle_failure_key(failure)),
            failure.clone(),
        );
        Ok(())
    }

    async fn lifecycle_effect_failures(
        &self,
        mob_id: &str,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<LifecycleEffectFailure>, MetadataStoreError> {
        let mut failures = self
            .lifecycle_failures
            .read()
            .await
            .values()
            .filter(|failure| failure.mob_id == mob_id)
            .cloned()
            .collect::<Vec<_>>();
        failures.sort_by(lifecycle_effect_failure_order);
        Ok(failures.into_iter().skip(skip).take(limit).collect())
    }

    async fn prune_lifecycle_effect_failures(
        &self,
        mob_id: &str,
        keep: usize,
    ) -> Result<usize, MetadataStoreError> {
        let mut stored = self.lifecycle_failures.write().await;
        let mut failures = stored
            .values()
            .filter(|failure| failure.mob_id == mob_id)
            .cloned()
            .collect::<Vec<_>>();
        failures.sort_by(lifecycle_effect_failure_order);
        let older = failures.len().saturating_sub(keep);
        for failure in &failures[..older] {
            stored.remove(&(failure.mob_id.clone(), lifecycle_failure_key(failure)));
        }
        Ok(older)
    }
}

/// SQLite-backed persistent metadata store.
///
/// Opens its own `rusqlite::Connection` to the supplied database path —
/// the same path the mob's `MobStorage` uses, but with a separate handle.
/// Cross-handle access is safe; meerkat #445's `notify`-based event-store
/// watcher already runs in this configuration. The `meta_metadata`
/// table is independent of meerkat-mob's own schema, so opening order
/// doesn't matter; in the shared file's migration ledger the table lives
/// under meta's own `meta-metadata` domain, co-tenanting meerkat-mob's
/// `mob` domain (the ledger keys strictly by domain name, so the two
/// crates stamp and migrate independently).
///
/// Schema:
/// ```text
/// CREATE TABLE meta_metadata (
///     mob_id  TEXT NOT NULL,
///     key     TEXT NOT NULL,
///     value   TEXT NOT NULL,
///     PRIMARY KEY (mob_id, key)
/// )
/// ```
///
/// The subscription cursor lives at `key = "subscription_cursor"`,
/// stored as a base-10 string for simple human inspection. Future
/// metadata fields land here under their own keys.
pub struct SqliteMetadataStore {
    conn: Mutex<Connection>,
    /// Database file path; `:memory:` for in-memory stores (where the
    /// per-operation fence guard degrades to a no-op).
    db_path: PathBuf,
}

const SUBSCRIPTION_CURSOR_KEY: &str = "subscription_cursor";

/// Key prefix of a member idle-retirement opt-in row: the member id follows
/// the prefix and the value is the JSON-serialized bound session and policy
/// (`StoredMemberIdleRetireOverride`).
const MEMBER_IDLE_RETIRE_KEY_PREFIX: &str = "member_idle_retire/";

fn member_idle_retire_key(member_id: &str) -> String {
    format!("{MEMBER_IDLE_RETIRE_KEY_PREFIX}{member_id}")
}

/// The runtime-metadata store's schema domain in the per-file migration
/// ledger. Migration 0001 is the historical one-table DDL.
const META_METADATA_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "meta-metadata",
    migrations: &[meerkat_sqlite::Migration {
        version: 1,
        name: "base-schema",
        apply: migration_0001_metadata_schema,
    }],
    initialize_current: migration_0001_metadata_schema,
    allowed_existing_versions: &[1],
    // Unledgered meta files are refused at open (below the 0.8.8 ledger
    // floor) and meta never runs the offline bridge, so no source
    // version is inferable.
    bridge_recoverable_versions: &[],
    released_predecessors: &[],
    owned_objects: &[meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: METADATA_TABLE,
    }],
    // The predecessor (pre-0.9) table name stays reserved: a file that
    // still carries it has not been adopted, and opening it must refuse
    // rather than start an empty table beside it.
    retired_objects: &[meerkat_sqlite::SchemaObject {
        kind: meerkat_sqlite::SchemaObjectKind::Table,
        name: crate::storage_predecessor::PREDECESSOR_METADATA_TABLE,
    }],
};

/// The runtime-metadata table name.
pub(crate) const METADATA_TABLE: &str = "meta_metadata";

/// Create the runtime-metadata table with the current DDL (shared with the
/// predecessor adoption so an adopted catalog matches a fresh one).
pub(crate) fn create_metadata_table(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    migration_0001_metadata_schema(tx)
}

fn migration_0001_metadata_schema(tx: &rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta_metadata (
            mob_id TEXT NOT NULL,
            key    TEXT NOT NULL,
            value  TEXT NOT NULL,
            PRIMARY KEY (mob_id, key)
        );",
    )
}

impl SqliteMetadataStore {
    /// Open (or create) a SQLite metadata store at `path`.
    ///
    /// `path` should typically be the same database the mob's `MobStorage`
    /// uses; the table is `meta_metadata` and won't collide with
    /// meerkat-mob's own tables.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MetadataStoreError> {
        let path = path.as_ref().to_path_buf();
        let mut conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
            .map_err(|err| MetadataStoreError::Io(format!("open: {err}")))?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &META_METADATA_DOMAIN)
            .map_err(|err| MetadataStoreError::Io(format!("schema: {err}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path: path,
        })
    }

    /// Open an in-memory SQLite store (for tests).
    pub fn in_memory() -> Result<Self, MetadataStoreError> {
        let mut conn = Connection::open_in_memory()
            .map_err(|err| MetadataStoreError::Io(format!("in-memory open: {err}")))?;
        meerkat_sqlite::apply_domain_migrations(&mut conn, &META_METADATA_DOMAIN)
            .map_err(|err| MetadataStoreError::Io(format!("schema: {err}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path: PathBuf::from(":memory:"),
        })
    }

    /// Per-operation maintenance-fence guard: the connection is held for
    /// the store's lifetime, so the fence cannot ride the open — every
    /// operation takes its own shared guard.
    fn operation_fence(&self) -> Result<meerkat_sqlite::OperationGuard, MetadataStoreError> {
        meerkat_sqlite::OperationGuard::for_database(&self.db_path)
            .map_err(|err| MetadataStoreError::Io(format!("operation fence: {err}")))
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, MetadataStoreError> {
        self.conn
            .lock()
            .map_err(|err| MetadataStoreError::Io(format!("connection mutex poisoned: {err}")))
    }
}

#[async_trait]
impl PersistentMetadataStore for SqliteMetadataStore {
    async fn get_subscription_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<u64>, MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT value FROM meta_metadata WHERE mob_id = ?1 AND key = ?2 LIMIT 1",
            )
            .map_err(|err| MetadataStoreError::Io(format!("prepare: {err}")))?;
        let value: Option<String> = stmt
            .query_row(rusqlite::params![mob_id, SUBSCRIPTION_CURSOR_KEY], |row| {
                row.get::<_, String>(0)
            })
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(MetadataStoreError::Io(format!("query: {other}"))),
            })?;
        match value {
            Some(s) => s
                .parse::<u64>()
                .map(Some)
                .map_err(|err| MetadataStoreError::Decode(format!("cursor parse: {err}"))),
            None => Ok(None),
        }
    }

    async fn set_subscription_cursor(
        &self,
        mob_id: &str,
        cursor: u64,
    ) -> Result<(), MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(mob_id, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![mob_id, SUBSCRIPTION_CURSOR_KEY, cursor.to_string()],
        )
        .map_err(|err| MetadataStoreError::Io(format!("upsert: {err}")))?;
        Ok(())
    }

    async fn load_member_idle_retire_overrides(
        &self,
    ) -> Result<Vec<MemberIdleRetireOverrideRecord>, MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        // A prefix compare, not LIKE: member ids may carry `_` and `%`.
        let mut stmt = conn
            .prepare_cached(
                "SELECT mob_id, key, value FROM meta_metadata \
                 WHERE substr(key, 1, ?1) = ?2 ORDER BY mob_id, key",
            )
            .map_err(|err| MetadataStoreError::Io(format!("prepare: {err}")))?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    MEMBER_IDLE_RETIRE_KEY_PREFIX.len(),
                    MEMBER_IDLE_RETIRE_KEY_PREFIX
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|err| MetadataStoreError::Io(format!("query: {err}")))?;
        let mut records = Vec::new();
        for row in rows {
            let (mob_id, key, value) =
                row.map_err(|err| MetadataStoreError::Io(format!("row: {err}")))?;
            let Some(member_id) = key.strip_prefix(MEMBER_IDLE_RETIRE_KEY_PREFIX) else {
                continue;
            };
            // One undecodable row (for instance a policy variant written by a
            // newer Meta before a rollback) costs only that member's
            // opt-in, never the whole restore.
            match serde_json::from_str::<StoredMemberIdleRetireOverride>(&value) {
                Ok(stored) => records.push(MemberIdleRetireOverrideRecord {
                    mob_id,
                    member_id: member_id.to_string(),
                    session_id: stored.session_id,
                    policy: stored.policy,
                    recorded_at: stored.recorded_at,
                }),
                Err(error) => tracing::warn!(
                    mob_id,
                    member_id,
                    error = %error,
                    "skipping an idle-retire opt-in row that cannot be decoded"
                ),
            }
        }
        Ok(records)
    }

    async fn set_member_idle_retire_override(
        &self,
        record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        let value = StoredMemberIdleRetireOverride::encode(record)?;
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(mob_id, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![
                record.mob_id,
                member_idle_retire_key(&record.member_id),
                value
            ],
        )
        .map_err(|err| MetadataStoreError::Io(format!("upsert: {err}")))?;
        Ok(())
    }

    async fn clear_member_idle_retire_override(
        &self,
        record: &MemberIdleRetireOverrideRecord,
    ) -> Result<(), MetadataStoreError> {
        // Match on the key and the DECODED session id, not the stored bytes:
        // rows written by other versions (another `recorded_at` shape) must
        // stay deletable. The DELETE then pins the exact value read, so a row
        // rewritten for a newer member in between is kept.
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        let key = member_idle_retire_key(&record.member_id);
        let stored: Option<String> = conn
            .query_row(
                "SELECT value FROM meta_metadata WHERE mob_id = ?1 AND key = ?2",
                rusqlite::params![record.mob_id, key],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(MetadataStoreError::Io(format!("query: {other}"))),
            })?;
        let Some(stored) = stored else {
            return Ok(());
        };
        let bound_to_record = serde_json::from_str::<StoredMemberIdleRetireSession>(&stored)
            .is_ok_and(|row| row.session_id == record.session_id);
        if !bound_to_record {
            return Ok(());
        }
        conn.execute(
            "DELETE FROM meta_metadata WHERE mob_id = ?1 AND key = ?2 AND value = ?3",
            rusqlite::params![record.mob_id, key, stored],
        )
        .map_err(|err| MetadataStoreError::Io(format!("delete: {err}")))?;
        Ok(())
    }

    // Identity records are ordinary rows of the `meta_metadata` table under
    // their own key prefixes: no new table, no schema version. A Meta that
    // predates them reads its rows by exact key or by its own prefixes only,
    // so it never meets these rows (rollback-safe).

    async fn identity_tool_record(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Result<Option<IdentityToolRecord>, MetadataStoreError> {
        let key = format!("{IDENTITY_TOOLS_KEY_PREFIX}{identity}");
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM meta_metadata WHERE mob_id = ?1 AND key = ?2",
                rusqlite::params![mob_id, key],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(MetadataStoreError::Io(format!("query: {other}"))),
            })?;
        value
            .map(|value| decode_identity_record(value.as_bytes()))
            .transpose()
    }

    async fn set_identity_tool_record(
        &self,
        record: &IdentityToolRecord,
    ) -> Result<(), MetadataStoreError> {
        let value = encode_identity_record(record)?;
        let key = format!("{IDENTITY_TOOLS_KEY_PREFIX}{}", record.identity);
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(mob_id, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![record.mob_id, key, value],
        )
        .map_err(|err| MetadataStoreError::Io(format!("upsert: {err}")))?;
        Ok(())
    }

    async fn load_identity_rotation_intents(
        &self,
        mob_id: &str,
    ) -> Result<Vec<IdentityRotationIntent>, MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        // A key range, not LIKE (identities may carry `_` and `%`) nor a
        // compare of each key's start, which no index serves.
        let mut stmt = conn
            .prepare_cached(
                "SELECT value FROM meta_metadata \
                 WHERE mob_id = ?1 AND key >= ?2 AND key < ?3 ORDER BY key",
            )
            .map_err(|err| MetadataStoreError::Io(format!("prepare: {err}")))?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    mob_id,
                    IDENTITY_ROTATION_KEY_PREFIX,
                    key_range_end(IDENTITY_ROTATION_KEY_PREFIX)
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(|err| MetadataStoreError::Io(format!("query: {err}")))?;
        let mut intents = Vec::new();
        for row in rows {
            let value = row.map_err(|err| MetadataStoreError::Io(format!("row: {err}")))?;
            intents.push(decode_identity_record(value.as_bytes())?);
        }
        Ok(intents)
    }

    async fn set_identity_rotation_intent(
        &self,
        intent: &IdentityRotationIntent,
    ) -> Result<(), MetadataStoreError> {
        let value = encode_identity_record(intent)?;
        let key = format!("{IDENTITY_ROTATION_KEY_PREFIX}{}", intent.identity);
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(mob_id, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![intent.mob_id, key, value],
        )
        .map_err(|err| MetadataStoreError::Io(format!("upsert: {err}")))?;
        Ok(())
    }

    async fn clear_identity_rotation_intent(
        &self,
        mob_id: &str,
        identity: &str,
    ) -> Result<(), MetadataStoreError> {
        let key = format!("{IDENTITY_ROTATION_KEY_PREFIX}{identity}");
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM meta_metadata WHERE mob_id = ?1 AND key = ?2",
            rusqlite::params![mob_id, key],
        )
        .map_err(|err| MetadataStoreError::Io(format!("delete: {err}")))?;
        Ok(())
    }

    // The lifecycle projection's cursor and intents are identity records as
    // well: rows under their own keys, read by exact key only.

    async fn lifecycle_projection_cursor(
        &self,
        mob_id: &str,
    ) -> Result<Option<LifecycleProjectionCursor>, MetadataStoreError> {
        self.identity_record_row(mob_id, LIFECYCLE_CURSOR_KEY)?
            .map(|value| decode_identity_record(value.as_bytes()))
            .transpose()
    }

    async fn set_lifecycle_projection_cursor(
        &self,
        cursor: &LifecycleProjectionCursor,
    ) -> Result<(), MetadataStoreError> {
        let value = encode_identity_record(cursor)?;
        self.upsert_identity_record_row(&cursor.mob_id, LIFECYCLE_CURSOR_KEY, &value)
    }

    async fn lifecycle_effect_intent(
        &self,
        mob_id: &str,
        identity: &str,
        generation: u64,
    ) -> Result<Option<LifecycleEffectIntent>, MetadataStoreError> {
        self.identity_record_row(mob_id, &lifecycle_intent_key(identity, generation))?
            .map(|value| decode_identity_record(value.as_bytes()))
            .transpose()
    }

    async fn set_lifecycle_effect_intent(
        &self,
        intent: &LifecycleEffectIntent,
    ) -> Result<(), MetadataStoreError> {
        let value = encode_identity_record(intent)?;
        self.upsert_identity_record_row(
            &intent.mob_id,
            &lifecycle_intent_key(&intent.identity, intent.generation),
            &value,
        )
    }

    async fn clear_lifecycle_effect_intent(
        &self,
        mob_id: &str,
        identity: &str,
        generation: u64,
    ) -> Result<(), MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM meta_metadata WHERE mob_id = ?1 AND key = ?2",
            rusqlite::params![mob_id, lifecycle_intent_key(identity, generation)],
        )
        .map_err(|err| MetadataStoreError::Io(format!("delete: {err}")))?;
        Ok(())
    }

    async fn lifecycle_effect_intents(
        &self,
        mob_id: &str,
    ) -> Result<Vec<LifecycleEffectIntent>, MetadataStoreError> {
        self.identity_record_rows(mob_id, LIFECYCLE_INTENT_KEY_PREFIX)?
            .iter()
            .map(|value| decode_identity_record(value.as_bytes()))
            .collect()
    }

    async fn record_lifecycle_effect_failure(
        &self,
        failure: &LifecycleEffectFailure,
    ) -> Result<(), MetadataStoreError> {
        let value = encode_identity_record(failure)?;
        self.upsert_identity_record_row(&failure.mob_id, &lifecycle_failure_key(failure), &value)
    }

    async fn lifecycle_effect_failures(
        &self,
        mob_id: &str,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<LifecycleEffectFailure>, MetadataStoreError> {
        let failures = self.lifecycle_effect_failures_of(mob_id)?;
        Ok(failures.into_iter().skip(skip).take(limit).collect())
    }

    async fn prune_lifecycle_effect_failures(
        &self,
        mob_id: &str,
        keep: usize,
    ) -> Result<usize, MetadataStoreError> {
        let failures = self.lifecycle_effect_failures_of(mob_id)?;
        let older = failures.len().saturating_sub(keep);
        if older == 0 {
            return Ok(0);
        }
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        let transaction = conn
            .unchecked_transaction()
            .map_err(|err| MetadataStoreError::Io(format!("begin: {err}")))?;
        for failure in &failures[..older] {
            transaction
                .execute(
                    "DELETE FROM meta_metadata WHERE mob_id = ?1 AND key = ?2",
                    rusqlite::params![mob_id, lifecycle_failure_key(failure)],
                )
                .map_err(|err| MetadataStoreError::Io(format!("delete: {err}")))?;
        }
        transaction
            .commit()
            .map_err(|err| MetadataStoreError::Io(format!("commit: {err}")))?;
        Ok(older)
    }
}

impl SqliteMetadataStore {
    /// Every lifecycle effect failure of `mob_id`, in the order
    /// [`PersistentMetadataStore::lifecycle_effect_failures`] reads them.
    fn lifecycle_effect_failures_of(
        &self,
        mob_id: &str,
    ) -> Result<Vec<LifecycleEffectFailure>, MetadataStoreError> {
        let mut failures = self
            .identity_record_rows(mob_id, LIFECYCLE_FAILURE_KEY_PREFIX)?
            .iter()
            .map(|value| decode_identity_record::<LifecycleEffectFailure>(value.as_bytes()))
            .collect::<Result<Vec<_>, _>>()?;
        failures.sort_by(lifecycle_effect_failure_order);
        Ok(failures)
    }

    /// The values of the `meta_metadata` rows of `mob_id` whose key starts
    /// with `prefix` (a key prefix ending with `/`), in key order.
    fn identity_record_rows(
        &self,
        mob_id: &str,
        prefix: &str,
    ) -> Result<Vec<String>, MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        // A key range, not LIKE (identities may carry `_` and `%`) nor a
        // compare of each key's start, which no index serves.
        let mut stmt = conn
            .prepare_cached(
                "SELECT value FROM meta_metadata \
                 WHERE mob_id = ?1 AND key >= ?2 AND key < ?3 ORDER BY key",
            )
            .map_err(|err| MetadataStoreError::Io(format!("prepare: {err}")))?;
        let rows = stmt
            .query_map(
                rusqlite::params![mob_id, prefix, key_range_end(prefix)],
                |row| row.get::<_, String>(0),
            )
            .map_err(|err| MetadataStoreError::Io(format!("query: {err}")))?;
        let mut values = Vec::new();
        for row in rows {
            values.push(row.map_err(|err| MetadataStoreError::Io(format!("row: {err}")))?);
        }
        Ok(values)
    }

    /// The value of the `meta_metadata` row `key` of `mob_id`, if any.
    fn identity_record_row(
        &self,
        mob_id: &str,
        key: &str,
    ) -> Result<Option<String>, MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT value FROM meta_metadata WHERE mob_id = ?1 AND key = ?2",
            rusqlite::params![mob_id, key],
            |row| row.get::<_, String>(0),
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(MetadataStoreError::Io(format!("query: {other}"))),
        })
    }

    /// Record (or replace) the `meta_metadata` row `key` of `mob_id`.
    fn upsert_identity_record_row(
        &self,
        mob_id: &str,
        key: &str,
        value: &str,
    ) -> Result<(), MetadataStoreError> {
        let _fence = self.operation_fence()?;
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3) \
             ON CONFLICT(mob_id, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![mob_id, key, value],
        )
        .map_err(|err| MetadataStoreError::Io(format!("upsert: {err}")))?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[tokio::test]
    async fn set_and_get_mob_labels() {
        let table = RuntimeMetadataTable::new();
        let scope = MetadataScope::Mob("mob-a".to_string());
        table
            .set_labels(scope.clone(), labels(&[("repo", "agents"), ("env", "dev")]))
            .await;
        let got = table.get_labels(&scope).await;
        assert_eq!(got.get("repo").map(String::as_str), Some("agents"));
        assert_eq!(got.get("env").map(String::as_str), Some("dev"));
    }

    #[tokio::test]
    async fn set_replaces_rather_than_merges() {
        let table = RuntimeMetadataTable::new();
        let scope = MetadataScope::Mob("mob-a".to_string());
        table
            .set_labels(scope.clone(), labels(&[("a", "1"), ("b", "2")]))
            .await;
        table.set_labels(scope.clone(), labels(&[("a", "9")])).await;
        let got = table.get_labels(&scope).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got.get("a").map(String::as_str), Some("9"));
        assert!(!got.contains_key("b"));
    }

    #[tokio::test]
    async fn delete_clears_entry() {
        let table = RuntimeMetadataTable::new();
        let scope = MetadataScope::Run("mob-a".to_string(), "run-1".to_string());
        table.set_labels(scope.clone(), labels(&[("k", "v")])).await;
        let prev = table.delete_labels(&scope).await;
        assert_eq!(prev.unwrap().get("k").map(String::as_str), Some("v"));
        let after = table.get_labels(&scope).await;
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn empty_set_clears_entry() {
        let table = RuntimeMetadataTable::new();
        let scope = MetadataScope::Mob("mob-a".to_string());
        table.set_labels(scope.clone(), labels(&[("k", "v")])).await;
        table.set_labels(scope.clone(), BTreeMap::new()).await;
        assert!(table.get_labels(&scope).await.is_empty());
    }

    #[tokio::test]
    async fn list_returns_mob_and_run_entries() {
        let table = RuntimeMetadataTable::new();
        let mob_scope = MetadataScope::Mob("mob-a".to_string());
        let run_scope = MetadataScope::Run("mob-a".to_string(), "run-1".to_string());
        let other_run = MetadataScope::Run("mob-b".to_string(), "run-1".to_string());
        table
            .set_labels(mob_scope.clone(), labels(&[("env", "dev")]))
            .await;
        table
            .set_labels(run_scope.clone(), labels(&[("trace", "abc")]))
            .await;
        table
            .set_labels(other_run, labels(&[("trace", "xyz")]))
            .await;

        let entries = table.list_labels_for_mob("mob-a").await;
        assert_eq!(entries.len(), 2);
        let scopes: Vec<&MetadataScope> = entries.iter().map(|(s, _)| s).collect();
        assert!(scopes.contains(&&mob_scope));
        assert!(scopes.contains(&&run_scope));
    }

    // ----- PersistentMetadataStore tests --------------------------------

    #[tokio::test]
    async fn in_memory_persistent_store_round_trip() {
        let store = InMemoryMetadataStore::new();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            None,
            "fresh store should have no cursor",
        );
        store.set_subscription_cursor("mob-a", 42).await.unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(42),
        );
        // Per-mob isolation.
        assert_eq!(store.get_subscription_cursor("mob-b").await.unwrap(), None,);
    }

    #[tokio::test]
    async fn in_memory_persistent_store_overwrite() {
        let store = InMemoryMetadataStore::new();
        store.set_subscription_cursor("m", 1).await.unwrap();
        store.set_subscription_cursor("m", 2).await.unwrap();
        assert_eq!(store.get_subscription_cursor("m").await.unwrap(), Some(2),);
    }

    #[tokio::test]
    async fn sqlite_persistent_store_round_trip() {
        let store = SqliteMetadataStore::in_memory().unwrap();
        assert_eq!(store.get_subscription_cursor("mob-a").await.unwrap(), None,);
        store.set_subscription_cursor("mob-a", 1234).await.unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(1234),
        );
        // Overwrite via UPSERT.
        store.set_subscription_cursor("mob-a", 9999).await.unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(9999),
        );
        // Per-mob isolation.
        store.set_subscription_cursor("mob-b", 5).await.unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(9999),
        );
        assert_eq!(
            store.get_subscription_cursor("mob-b").await.unwrap(),
            Some(5),
        );
    }

    #[tokio::test]
    async fn sqlite_store_persists_across_handles() {
        // The whole point of SQLite-backed persistence: a fresh handle to
        // the same DB sees writes from the previous handle. We can't drop
        // and reopen an in-memory DB (it disappears with the connection),
        // so write to a tempfile, drop, reopen.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta-metadata.sqlite");
        {
            let store = SqliteMetadataStore::open(&path).unwrap();
            store.set_subscription_cursor("mob-x", 7777).await.unwrap();
        }
        // Reopen.
        let store = SqliteMetadataStore::open(&path).unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-x").await.unwrap(),
            Some(7777),
            "cursor should survive handle drop",
        );
    }

    #[tokio::test]
    async fn fresh_store_stamps_meta_metadata_domain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta-metadata.sqlite");
        let store = SqliteMetadataStore::open(&path).unwrap();
        store.set_subscription_cursor("mob-a", 1).await.unwrap();
        let probe = Connection::open(&path).unwrap();
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-metadata").unwrap(),
            Some(1)
        );
    }

    fn tool_record(mob_id: &str, identity: &str, record_revision: u64) -> IdentityToolRecord {
        IdentityToolRecord {
            mob_id: mob_id.to_string(),
            identity: identity.to_string(),
            session_id: meerkat_core::types::SessionId::new(),
            tools: vec!["fetch".to_string(), "search".to_string()],
            record_revision,
            published_at_ms: 1_700_000_000_000,
        }
    }

    fn rotation(mob_id: &str, identity: &str) -> IdentityRotationIntent {
        IdentityRotationIntent {
            mob_id: mob_id.to_string(),
            identity: identity.to_string(),
            verb: "reset".to_string(),
            from_generation: Some(3),
            recorded_at_ms: 1_700_000_000_000,
        }
    }

    /// A tool record stored before its counter was renamed still reads: its
    /// `generation` is the record's revision.
    #[test]
    fn a_tool_record_stored_with_its_earlier_counter_name_still_reads() {
        let session_id = meerkat_core::types::SessionId::new();
        let stored = serde_json::json!({
            "v": IDENTITY_RECORD_VERSION,
            "mob_id": "mob-a",
            "identity": "personal:alice",
            "session_id": session_id.to_string(),
            "tools": ["fetch"],
            "generation": 7,
            "published_at_ms": 1_700_000_000_000_u64,
        });
        let record: IdentityToolRecord =
            decode_identity_record(stored.to_string().as_bytes()).expect("the record reads");
        assert_eq!(record.record_revision, 7);
        assert_eq!(record.session_id, session_id);
        let reencoded = encode_identity_record(&record).expect("the record encodes");
        assert!(
            reencoded.contains("\"record_revision\":7"),
            "a record is written under its current name: {reencoded}"
        );
    }

    /// R121 and R114: an identity's tool record and rotation intent round
    /// trip per mob, a later record replaces the earlier one, and a cleared
    /// intent is gone, on the in-memory and the SQLite store alike.
    #[tokio::test]
    async fn identity_records_round_trip_per_mob() {
        let dir = tempfile::tempdir().unwrap();
        let sqlite = SqliteMetadataStore::open(dir.path().join("meta.sqlite")).unwrap();
        let memory = InMemoryMetadataStore::new();
        let stores: [&dyn PersistentMetadataStore; 2] = [&sqlite, &memory];
        for store in stores {
            assert_eq!(
                store
                    .identity_tool_record("mob-a", "personal:alice")
                    .await
                    .unwrap(),
                None
            );
            store
                .set_identity_tool_record(&tool_record("mob-a", "personal:alice", 1))
                .await
                .unwrap();
            let newer = tool_record("mob-a", "personal:alice", 2);
            store.set_identity_tool_record(&newer).await.unwrap();
            assert_eq!(
                store
                    .identity_tool_record("mob-a", "personal:alice")
                    .await
                    .unwrap(),
                Some(newer)
            );
            assert_eq!(
                store
                    .identity_tool_record("mob-b", "personal:alice")
                    .await
                    .unwrap(),
                None,
                "records are per mob"
            );

            let intent = rotation("mob-a", "personal:alice");
            store.set_identity_rotation_intent(&intent).await.unwrap();
            store
                .set_identity_rotation_intent(&rotation("mob-b", "personal:bob"))
                .await
                .unwrap();
            assert_eq!(
                store.load_identity_rotation_intents("mob-a").await.unwrap(),
                vec![intent]
            );
            store
                .clear_identity_rotation_intent("mob-a", "personal:alice")
                .await
                .unwrap();
            assert!(
                store
                    .load_identity_rotation_intents("mob-a")
                    .await
                    .unwrap()
                    .is_empty()
            );
            store
                .clear_identity_rotation_intent("mob-a", "personal:alice")
                .await
                .expect("clearing an absent intent is no error");
        }
    }

    fn effect_intent(
        mob_id: &str,
        identity: &str,
        generation: u64,
        verb: LifecycleEffectVerb,
    ) -> LifecycleEffectIntent {
        LifecycleEffectIntent::new(
            mob_id,
            "incarnation-1",
            identity,
            generation,
            verb,
            Some(meerkat_core::types::SessionId::new()),
            1_700_000_000_000 + generation,
        )
    }

    /// The lifecycle projection's records are per mob, survive a reopen of
    /// a SQLite store, and an intent is keyed by its identity and the
    /// generation it ends: an identity's next generation is another intent,
    /// and an identity whose name looks like a key's tail is another one.
    #[tokio::test]
    async fn lifecycle_projection_records_round_trip_per_mob_and_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.sqlite");
        let sqlite = SqliteMetadataStore::open(&path).unwrap();
        let memory = InMemoryMetadataStore::new();
        let stores: [&dyn PersistentMetadataStore; 2] = [&sqlite, &memory];
        let mut cursor =
            LifecycleProjectionCursor::new("mob-a", "incarnation-1", 41, 1_700_000_000_000);
        cursor.backlog = vec![LifecycleProjectionBacklog::new(7, 12)];
        cursor.uncleared = vec![LifecycleIntentKey::new("personal:alice", 2)];
        let unlisted = effect_intent("mob-a", "personal:alice", 3, LifecycleEffectVerb::Unlisted);
        let deleted = effect_intent("mob-a", "personal:alice", 4, LifecycleEffectVerb::Delete);
        let lookalike = effect_intent("mob-a", "3/personal:alice", 0, LifecycleEffectVerb::Reset);
        for store in stores {
            assert_eq!(
                store.lifecycle_projection_cursor("mob-a").await.unwrap(),
                None
            );
            store
                .set_lifecycle_projection_cursor(&cursor)
                .await
                .unwrap();
            assert_eq!(
                store.lifecycle_projection_cursor("mob-a").await.unwrap(),
                Some(cursor.clone())
            );
            assert_eq!(
                store.lifecycle_projection_cursor("mob-b").await.unwrap(),
                None,
                "cursors are per mob"
            );
            for intent in [&unlisted, &deleted, &lookalike] {
                store.set_lifecycle_effect_intent(intent).await.unwrap();
            }
            assert_eq!(
                store
                    .lifecycle_effect_intent("mob-a", "personal:alice", 3)
                    .await
                    .unwrap(),
                Some(unlisted.clone())
            );
            assert_eq!(
                store
                    .lifecycle_effect_intent("mob-a", "personal:alice", 4)
                    .await
                    .unwrap(),
                Some(deleted.clone())
            );
            assert_eq!(
                store
                    .lifecycle_effect_intent("mob-a", "3/personal:alice", 0)
                    .await
                    .unwrap(),
                Some(lookalike.clone())
            );
            assert_eq!(
                store
                    .lifecycle_effect_intent("mob-b", "personal:alice", 3)
                    .await
                    .unwrap(),
                None,
                "intents are per mob"
            );
            store
                .clear_lifecycle_effect_intent("mob-a", "personal:alice", 3)
                .await
                .unwrap();
            assert_eq!(
                store
                    .lifecycle_effect_intent("mob-a", "personal:alice", 3)
                    .await
                    .unwrap(),
                None
            );
            assert_eq!(
                store
                    .lifecycle_effect_intent("mob-a", "personal:alice", 4)
                    .await
                    .unwrap(),
                Some(deleted.clone()),
                "clearing one generation keeps the next"
            );
            store
                .clear_lifecycle_effect_intent("mob-a", "personal:alice", 3)
                .await
                .expect("clearing an absent intent is no error");
        }
        drop(sqlite);
        let reopened = SqliteMetadataStore::open(&path).unwrap();
        assert_eq!(
            reopened.lifecycle_projection_cursor("mob-a").await.unwrap(),
            Some(cursor),
            "the cursor survives a reopen"
        );
        assert_eq!(
            reopened
                .lifecycle_effect_intent("mob-a", "personal:alice", 4)
                .await
                .unwrap(),
            Some(deleted)
        );
    }

    /// A mob's lifecycle effect intents list with the mob's own only, and
    /// the failures the projection records list oldest first, per mob,
    /// keyed by generation, effect and identity, and survive a reopen.
    #[tokio::test]
    async fn lifecycle_intents_list_and_failures_round_trip_per_mob() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.sqlite");
        let sqlite = SqliteMetadataStore::open(&path).unwrap();
        let memory = InMemoryMetadataStore::new();
        let stores: [&dyn PersistentMetadataStore; 2] = [&sqlite, &memory];
        let hook = LifecycleEffectFailure::new(
            "mob-a",
            "incarnation-1",
            12,
            "personal:alice",
            3,
            LifecycleEffectKind::RetirementHook,
            3,
            "timed out",
            1_700_000_000_001,
        )
        .with_session(Some(meerkat_core::types::SessionId::new()));
        let interview = LifecycleEffectFailure::new(
            "mob-a",
            "incarnation-1",
            12,
            "personal:alice",
            3,
            LifecycleEffectKind::ExitInterview,
            3,
            "the queue refused the write",
            1_700_000_000_000,
        );
        for store in stores {
            for intent in [
                effect_intent("mob-a", "personal:alice", 3, LifecycleEffectVerb::Unlisted),
                effect_intent("mob-a", "personal:bob", 1, LifecycleEffectVerb::RoleMove),
                effect_intent("mob-b", "personal:alice", 3, LifecycleEffectVerb::Reset),
            ] {
                store.set_lifecycle_effect_intent(&intent).await.unwrap();
            }
            let mut listed = store
                .lifecycle_effect_intents("mob-a")
                .await
                .unwrap()
                .into_iter()
                .map(|intent| (intent.identity, intent.generation, intent.verb))
                .collect::<Vec<_>>();
            listed.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
            assert_eq!(
                listed,
                vec![
                    (
                        "personal:alice".to_string(),
                        3,
                        LifecycleEffectVerb::Unlisted
                    ),
                    ("personal:bob".to_string(), 1, LifecycleEffectVerb::RoleMove),
                ]
            );
            store.record_lifecycle_effect_failure(&hook).await.unwrap();
            store
                .record_lifecycle_effect_failure(&interview)
                .await
                .unwrap();
            assert_eq!(
                store
                    .lifecycle_effect_failures("mob-a", 0, usize::MAX)
                    .await
                    .unwrap(),
                vec![interview.clone(), hook.clone()],
                "one record per effect, oldest first"
            );
            assert!(
                store
                    .lifecycle_effect_failures("mob-b", 0, usize::MAX)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        drop(sqlite);
        let reopened = SqliteMetadataStore::open(&path).unwrap();
        assert_eq!(
            reopened
                .lifecycle_effect_failures("mob-a", 0, usize::MAX)
                .await
                .unwrap(),
            vec![interview, hook],
            "the failures survive a reopen"
        );
    }

    /// A mob's lifecycle effect failures read in pages, oldest first, and a
    /// prune keeps the latest of them and leaves another mob's alone. The
    /// identity records read by key range take in the rows under their own
    /// prefix only, whatever sorts right next to it.
    #[tokio::test]
    async fn lifecycle_failures_read_in_pages_and_prune_to_the_latest() {
        let dir = tempfile::tempdir().unwrap();
        let sqlite = SqliteMetadataStore::open(dir.path().join("meta.sqlite")).unwrap();
        let memory = InMemoryMetadataStore::new();
        let stores: [&dyn PersistentMetadataStore; 2] = [&sqlite, &memory];
        let failure = |mob_id: &str, index: u64| {
            LifecycleEffectFailure::new(
                mob_id,
                "incarnation-1",
                index + 1,
                format!("personal:failed-{index}"),
                0,
                LifecycleEffectKind::AfterCreate,
                3,
                "timed out",
                1_700_000_000_000 + index,
            )
        };
        for store in stores {
            for index in (0..5).rev() {
                store
                    .record_lifecycle_effect_failure(&failure("mob-a", index))
                    .await
                    .unwrap();
            }
            store
                .record_lifecycle_effect_failure(&failure("mob-b", 0))
                .await
                .unwrap();
            assert_eq!(
                store
                    .lifecycle_effect_failures("mob-a", 0, 2)
                    .await
                    .unwrap(),
                vec![failure("mob-a", 0), failure("mob-a", 1)]
            );
            assert_eq!(
                store
                    .lifecycle_effect_failures("mob-a", 2, 2)
                    .await
                    .unwrap(),
                vec![failure("mob-a", 2), failure("mob-a", 3)]
            );
            assert_eq!(
                store
                    .lifecycle_effect_failures("mob-a", 4, 2)
                    .await
                    .unwrap(),
                vec![failure("mob-a", 4)]
            );
            assert_eq!(
                store
                    .prune_lifecycle_effect_failures("mob-a", 2)
                    .await
                    .unwrap(),
                3,
                "the three oldest go"
            );
            assert_eq!(
                store
                    .lifecycle_effect_failures("mob-a", 0, usize::MAX)
                    .await
                    .unwrap(),
                vec![failure("mob-a", 3), failure("mob-a", 4)]
            );
            assert_eq!(
                store
                    .prune_lifecycle_effect_failures("mob-a", 2)
                    .await
                    .unwrap(),
                0
            );
            assert_eq!(
                store
                    .lifecycle_effect_failures("mob-b", 0, usize::MAX)
                    .await
                    .unwrap(),
                vec![failure("mob-b", 0)],
                "another mob's failures stay"
            );
        }
        {
            let conn = sqlite.lock_conn().unwrap();
            for key in ["lifecycle_intent", "lifecycle_intent0"] {
                conn.execute(
                    "INSERT INTO meta_metadata (mob_id, key, value) VALUES ('mob-a', ?1, 'none')",
                    rusqlite::params![key],
                )
                .unwrap();
            }
        }
        assert!(
            sqlite
                .lifecycle_effect_intents("mob-a")
                .await
                .unwrap()
                .is_empty(),
            "a row beside the prefix is no intent"
        );
    }

    /// R117, the half this build can prove: the identity records are rows
    /// of the existing `meta_metadata` table under their own key prefixes,
    /// the metadata domain stays at version 1 (no schema change a Meta 0.9.5
    /// would refuse as from the future), and the reads an earlier Meta makes
    /// (its subscription cursor, its idle-retirement opt-ins) never meet
    /// them. The other half, a Meta 0.9.5 binary opening such a store, is the
    /// validation owner's cross-version run.
    #[tokio::test]
    async fn r117_identity_records_leave_the_schema_and_earlier_reads_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.sqlite");
        let store = SqliteMetadataStore::open(&path).unwrap();
        store.set_subscription_cursor("mob-a", 7).await.unwrap();
        store
            .set_identity_tool_record(&tool_record("mob-a", "personal:alice", 1))
            .await
            .unwrap();
        store
            .set_identity_rotation_intent(&rotation("mob-a", "personal:alice"))
            .await
            .unwrap();

        let probe = Connection::open(&path).unwrap();
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-metadata").unwrap(),
            Some(1),
            "no schema change"
        );
        let mut keys = probe
            .prepare("SELECT key FROM meta_metadata WHERE mob_id = 'mob-a' ORDER BY key")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        keys.retain(|key| key != "subscription_cursor");
        assert_eq!(
            keys,
            vec![
                "identity_rotation/personal:alice".to_string(),
                "identity_tools/personal:alice".to_string(),
            ]
        );
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(7)
        );
        assert!(
            store
                .load_member_idle_retire_overrides()
                .await
                .unwrap()
                .is_empty(),
            "an earlier Meta's prefix read never meets the identity records"
        );
    }

    /// A pre-ledger file (bare meta_metadata table, no meerkat_schema row)
    /// is refused typed at open with its rows left untouched and no ledger
    /// stamped: pre-ledger corpora are below the mobkit 0.8.8 floor, and the
    /// 0.8.11 reset retired silent pre-floor convergence (this test pinned
    /// that convergence until then).
    #[tokio::test]
    async fn legacy_metadata_file_is_refused_with_rows_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta-metadata.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta_metadata (
                    mob_id TEXT NOT NULL,
                    key    TEXT NOT NULL,
                    value  TEXT NOT NULL,
                    PRIMARY KEY (mob_id, key)
                );
                INSERT INTO meta_metadata (mob_id, key, value)
                    VALUES ('mob-legacy', 'subscription_cursor', '314');",
            )
            .unwrap();
        }
        assert!(
            SqliteMetadataStore::open(&path).is_err(),
            "opening a pre-ledger metadata database must refuse typed: unledgered owned \
             tables are below the mobkit 0.8.8 floor and must never be silently converged"
        );
        let probe = Connection::open(&path).unwrap();
        let preserved: String = probe
            .query_row(
                "SELECT value FROM meta_metadata \
                 WHERE mob_id = 'mob-legacy' AND key = 'subscription_cursor'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            preserved, "314",
            "the refusal must leave legacy rows untouched"
        );
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-metadata").unwrap(),
            None,
            "a refused open must not stamp the ledger"
        );
    }

    /// The metadata table co-tenants the same database file as meerkat-mob's
    /// MobStorage. The per-file ledger keys strictly by domain, so the two
    /// crates' domains (`mob` and `meta-metadata`) must coexist in one
    /// `meerkat_schema` table without clobbering each other.
    #[tokio::test]
    async fn metadata_and_mob_domains_cotenant_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mob.sqlite3");
        let _mob = meerkat_mob::MobStorage::persistent(&path).expect("mob storage");
        let store = SqliteMetadataStore::open(&path).expect("metadata store");
        store.set_subscription_cursor("mob-a", 42).await.unwrap();
        assert_eq!(
            store.get_subscription_cursor("mob-a").await.unwrap(),
            Some(42)
        );
        let probe = Connection::open(&path).unwrap();
        let mob_version = meerkat_sqlite::domain_version(&probe, "mob").unwrap();
        assert!(
            mob_version.is_some_and(|version| version >= 1),
            "meerkat-mob's own domain row must be present: {mob_version:?}"
        );
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-metadata").unwrap(),
            Some(1),
            "meta's domain row must coexist with meerkat-mob's"
        );
    }

    #[tokio::test]
    async fn run_scope_distinguishes_mobs() {
        let table = RuntimeMetadataTable::new();
        let scope_a = MetadataScope::Run("mob-a".to_string(), "run-1".to_string());
        let scope_b = MetadataScope::Run("mob-b".to_string(), "run-1".to_string());
        table
            .set_labels(scope_a.clone(), labels(&[("k", "a")]))
            .await;
        table
            .set_labels(scope_b.clone(), labels(&[("k", "b")]))
            .await;
        assert_eq!(
            table
                .get_labels(&scope_a)
                .await
                .get("k")
                .map(String::as_str),
            Some("a")
        );
        assert_eq!(
            table
                .get_labels(&scope_b)
                .await
                .get("k")
                .map(String::as_str),
            Some("b")
        );
    }

    fn idle_record(
        mob_id: &str,
        member_id: &str,
        session_id: &meerkat_core::types::SessionId,
        policy: crate::mob_handle_runtime::DelegateIdleRetireOverride,
    ) -> MemberIdleRetireOverrideRecord {
        MemberIdleRetireOverrideRecord {
            mob_id: mob_id.to_string(),
            member_id: member_id.to_string(),
            session_id: session_id.clone(),
            policy,
            recorded_at: chrono::DateTime::from_timestamp(1_790_000_000, 0)
                .expect("fixed timestamp"),
        }
    }

    /// Opt-ins written by one process are read back by the next one against
    /// the same file, bound sessions included, member ids with SQL wildcard
    /// characters included, and other metadata rows (the subscription
    /// cursor) never leak in.
    #[tokio::test]
    async fn sqlite_member_idle_retire_overrides_survive_reopen() {
        use crate::mob_handle_runtime::DelegateIdleRetireOverride;
        use meerkat_core::types::SessionId;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite3");
        let (s1, s2, s3) = (SessionId::new(), SessionId::new(), SessionId::new());
        let fork_child = idle_record(
            "mob-a",
            "fork_child%1",
            &s1,
            DelegateIdleRetireOverride::Seconds(300),
        );
        let helper = idle_record("mob-b", "helper", &s2, DelegateIdleRetireOverride::Disabled);
        let newer_fork_2 = idle_record(
            "mob-a",
            "fork-2",
            &s3,
            DelegateIdleRetireOverride::RuntimeDefault,
        );
        {
            let store = SqliteMetadataStore::open(&path).expect("open metadata store");
            store
                .set_subscription_cursor("mob-a", 7)
                .await
                .expect("cursor");
            store
                .set_member_idle_retire_override(&fork_child)
                .await
                .expect("set fork child");
            store
                .set_member_idle_retire_override(&helper)
                .await
                .expect("set helper");
            let older_fork_2 = idle_record(
                "mob-a",
                "fork-2",
                &s1,
                DelegateIdleRetireOverride::Seconds(5),
            );
            store
                .set_member_idle_retire_override(&older_fork_2)
                .await
                .expect("set fork-2");
            // A later member under the same id replaces the row...
            store
                .set_member_idle_retire_override(&newer_fork_2)
                .await
                .expect("replace fork-2");
            // ...and clearing the OLDER opt-in must not remove the newer one.
            store
                .clear_member_idle_retire_override(&older_fork_2)
                .await
                .expect("clearing a superseded opt-in is not an error");
            let gone = idle_record("mob-b", "gone", &s2, DelegateIdleRetireOverride::Seconds(1));
            store
                .set_member_idle_retire_override(&gone)
                .await
                .expect("set gone");
            store
                .clear_member_idle_retire_override(&gone)
                .await
                .expect("clear gone");
            store
                .clear_member_idle_retire_override(&gone)
                .await
                .expect("clearing an absent opt-in is not an error");
        }

        let reopened = SqliteMetadataStore::open(&path).expect("reopen metadata store");
        assert_eq!(
            reopened
                .load_member_idle_retire_overrides()
                .await
                .expect("load"),
            vec![newer_fork_2, fork_child, helper]
        );
        assert_eq!(
            reopened
                .get_subscription_cursor("mob-a")
                .await
                .expect("cursor"),
            Some(7)
        );
    }

    /// One undecodable row (a policy variant from a newer Meta, say) is
    /// skipped; the rest of the restore still loads.
    #[tokio::test]
    async fn sqlite_member_idle_retire_load_skips_undecodable_rows() {
        use crate::mob_handle_runtime::DelegateIdleRetireOverride;
        use meerkat_core::types::SessionId;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite3");
        let store = SqliteMetadataStore::open(&path).expect("open metadata store");
        let good = idle_record(
            "mob-a",
            "fork-1",
            &SessionId::new(),
            DelegateIdleRetireOverride::Seconds(300),
        );
        store
            .set_member_idle_retire_override(&good)
            .await
            .expect("set good row");
        {
            let conn = store.lock_conn().expect("connection");
            conn.execute(
                "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    "mob-a",
                    member_idle_retire_key("from-the-future"),
                    r#"{"session_id":"not-a-session","policy":"hibernate"}"#
                ],
            )
            .expect("insert undecodable row");
        }
        assert_eq!(
            store
                .load_member_idle_retire_overrides()
                .await
                .expect("load"),
            vec![good]
        );
    }

    #[tokio::test]
    async fn in_memory_member_idle_retire_overrides_set_replace_and_clear() {
        use crate::mob_handle_runtime::DelegateIdleRetireOverride;
        use meerkat_core::types::SessionId;

        let store = InMemoryMetadataStore::new();
        let (s1, s2) = (SessionId::new(), SessionId::new());
        let kept = idle_record(
            "mob-a",
            "fork-1",
            &s1,
            DelegateIdleRetireOverride::Seconds(300),
        );
        store
            .set_member_idle_retire_override(&kept)
            .await
            .expect("set");
        let older = idle_record(
            "mob-a",
            "fork-2",
            &s1,
            DelegateIdleRetireOverride::Seconds(5),
        );
        let newer = idle_record(
            "mob-a",
            "fork-2",
            &s2,
            DelegateIdleRetireOverride::RuntimeDefault,
        );
        store
            .set_member_idle_retire_override(&older)
            .await
            .expect("set");
        store
            .set_member_idle_retire_override(&newer)
            .await
            .expect("replace");
        store
            .clear_member_idle_retire_override(&older)
            .await
            .expect("stale clear");
        let mut loaded = store
            .load_member_idle_retire_overrides()
            .await
            .expect("load");
        loaded.sort_by(|a, b| a.member_id.cmp(&b.member_id));
        assert_eq!(loaded, vec![kept.clone(), newer.clone()]);
        store
            .clear_member_idle_retire_override(&newer)
            .await
            .expect("clear");
        assert_eq!(
            store
                .load_member_idle_retire_overrides()
                .await
                .expect("load"),
            vec![kept]
        );
    }

    /// Review of #447: a release deletes a row by its key and the session it
    /// is bound to, whatever version wrote it. A row written by #442 (no
    /// `recorded_at`) or by a build that stored extra fields is deletable; a
    /// row bound to another session (a newer member) is kept.
    #[tokio::test]
    async fn sqlite_member_idle_retire_rows_from_other_versions_are_deletable() {
        use crate::mob_handle_runtime::DelegateIdleRetireOverride;
        use meerkat_core::types::SessionId;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite3");
        let store = SqliteMetadataStore::open(&path).expect("open metadata store");
        let (old_format, new_format, kept) = (SessionId::new(), SessionId::new(), SessionId::new());
        {
            let conn = store.lock_conn().expect("connection");
            for (member, value) in [
                (
                    "written-by-442",
                    format!(r#"{{"session_id":"{old_format}","policy":{{"seconds":300}}}}"#),
                ),
                (
                    "written-with-extras",
                    format!(
                        r#"{{"session_id":"{new_format}","policy":"runtime_default","recorded_at":"2026-09-25T20:00:00Z","carrying":true}}"#
                    ),
                ),
                (
                    "newer-member",
                    format!(r#"{{"session_id":"{kept}","policy":"disabled"}}"#),
                ),
            ] {
                conn.execute(
                    "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3)",
                    rusqlite::params!["mob-a", member_idle_retire_key(member), value],
                )
                .expect("insert row");
            }
        }
        let loaded = store
            .load_member_idle_retire_overrides()
            .await
            .expect("load");
        assert_eq!(loaded.len(), 3, "every version's row decodes: {loaded:?}");
        for record in loaded
            .iter()
            .filter(|record| record.member_id != "newer-member")
        {
            store
                .clear_member_idle_retire_override(record)
                .await
                .expect("clear");
        }
        // A release for a gone instance of `newer-member` keeps the row.
        store
            .clear_member_idle_retire_override(&idle_record(
                "mob-a",
                "newer-member",
                &SessionId::new(),
                DelegateIdleRetireOverride::Disabled,
            ))
            .await
            .expect("stale clear");
        let remaining = store
            .load_member_idle_retire_overrides()
            .await
            .expect("load");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].member_id, "newer-member");
        assert_eq!(remaining[0].session_id, kept);
    }
}
