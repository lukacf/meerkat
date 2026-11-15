//! Async operation types for Meerkat
//!
//! Unified abstraction for tool calls, shell commands, and delegated branches.

use crate::budget::BudgetLimits;
use crate::error::ToolError;
use crate::session::DeferredToolLoadAuthority;
use crate::types::{Message, ToolNameSet};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for an operation
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OperationId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

/// Wait policy for async operations.
///
/// Determines whether an operation blocks the turn boundary (`Barrier`) or runs
/// independently (`Detached`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitPolicy {
    /// Operation must complete before `ToolCallsResolved` can fire.
    Barrier,
    /// Operation runs independently and does not block the turn.
    Detached,
}

/// Typed async operation reference carrying an operation ID and its wait policy.
///
/// Replaces raw `OperationId` sequences in the TurnExecution machine state to
/// enable barrier-aware scheduling. Only `Barrier` ops block the turn boundary;
/// `Detached` ops are recorded but do not gate `ToolCallsResolved`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AsyncOpRef {
    pub operation_id: OperationId,
    pub wait_policy: WaitPolicy,
}

impl WaitPolicy {
    /// Normal tool-call operations that must complete before the turn boundary.
    pub fn barrier() -> Self {
        Self::Barrier
    }

    /// Background or mob-child operations that run independently of the turn.
    pub fn detached() -> Self {
        Self::Detached
    }
}

impl AsyncOpRef {
    /// Create a barrier op ref — blocks the turn boundary until resolved.
    pub fn barrier(operation_id: OperationId) -> Self {
        Self {
            operation_id,
            wait_policy: WaitPolicy::barrier(),
        }
    }

    /// Create a detached op ref — runs independently, does not block the turn.
    pub fn detached(operation_id: OperationId) -> Self {
        Self {
            operation_id,
            wait_policy: WaitPolicy::detached(),
        }
    }
}

/// Outcome of a tool dispatch, separating transcript data from execution metadata.
///
/// `result` is what the model sees (conversation/transcript). `async_ops` is
/// what the runtime scheduler sees (barrier/detached classification). This
/// prevents hooks, persistence, and message serialization from accidentally
/// owning barrier semantics.
/// Typed session-level effect produced by tool dispatch.
///
/// Tools that need to mutate session-owned durable state (e.g., mob authority)
/// must NOT call `SessionService` methods from inside dispatch. Instead they
/// return typed effects here, and the turn owner (agent loop) merges and commits
/// them after the parallel tool batch completes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect_type", rename_all = "snake_case")]
pub enum SessionEffect {
    /// Replace the session's canonical mob operator authority projection.
    ///
    /// The replacement context is constructible only by generated machine
    /// authority. The turn owner rejects deserialized/unsealed effects before
    /// installing the durable projection and does not widen scope from local
    /// effect fields.
    ReplaceMobToolAuthorityContext {
        authority_context: crate::service::MobToolAuthorityContext,
    },
    /// Record durable deferred-tool requests for subsequent boundaries.
    RequestDeferredTools {
        authorities: Vec<DeferredToolLoadAuthority>,
    },
    /// Append durable assistant blocks produced by a tool after the tool has
    /// committed any external payloads (for example generated image blobs).
    AppendAssistantBlocks {
        blocks: Vec<crate::types::AssistantBlock>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ToolDispatchTerminalErrorKind {
    NotFound,
    Unavailable,
    InvalidArguments,
    ExecutionFailed,
    Timeout,
    AccessDenied,
    AuthorizationRefused,
    OperationObservationUnavailable,
    OperationAuthorizationUnavailable,
    PolicyDenied,
    PolicyIndeterminate,
    Other,
    CallbackPending,
    ConfinementRefused,
    HookDenied,
    // Appended so released ordinals stay stable. A plain comment, not a doc
    // comment: a documented unit variant would split this schema enum.
    OutcomeUncertain,
    ReviewUnsatisfied,
    ReviewUnavailable,
    HookLaunchRefused,
}

impl From<&ToolError> for ToolDispatchTerminalErrorKind {
    fn from(error: &ToolError) -> Self {
        match error {
            ToolError::NotFound { .. } => Self::NotFound,
            ToolError::Unavailable { .. } => Self::Unavailable,
            ToolError::InvalidArguments { .. } => Self::InvalidArguments,
            ToolError::ExecutionFailed { .. } | ToolError::ExecutionFailedWithData { .. } => {
                Self::ExecutionFailed
            }
            ToolError::Timeout { .. } | ToolError::InactivityTimeout { .. } => Self::Timeout,
            ToolError::AccessDenied { .. } => Self::AccessDenied,
            ToolError::OutcomeUncertain { .. } => Self::OutcomeUncertain,
            ToolError::AuthorizationRefused { .. } => Self::AuthorizationRefused,
            ToolError::ConfinementRefused { .. } => Self::ConfinementRefused,
            ToolError::HookDenied { .. } => Self::HookDenied,
            ToolError::HookLaunchRefused { .. } => Self::HookLaunchRefused,
            ToolError::OperationObservationUnavailable => Self::OperationObservationUnavailable,
            ToolError::OperationAuthorizationUnavailable => Self::OperationAuthorizationUnavailable,
            ToolError::ReviewUnsatisfied { .. } => Self::ReviewUnsatisfied,
            ToolError::ReviewUnavailable { .. } => Self::ReviewUnavailable,
            ToolError::PolicyDenied { .. } => Self::PolicyDenied,
            ToolError::PolicyIndeterminate { .. } => Self::PolicyIndeterminate,
            ToolError::Other(_) => Self::Other,
            ToolError::CallbackPending { .. } => Self::CallbackPending,
            ToolError::WithSettlementFailures { .. } => Self::from(error.primary_error()),
        }
    }
}

/// Which configured admission reported a settlement failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ToolDispatchAdmissionSource {
    ConfiguredGate,
    ContextGate,
    AuthorizationAudit,
}

/// A diagnostic that accompanies, and never replaces, the physical result.
///
/// This contains no error text, tool arguments, credentials or execution
/// authority. The admission owner retains any exact internal failure and the
/// selected physical outcome until its generated settlement succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ToolDispatchSettlementFailure {
    pub admission_source: ToolDispatchAdmissionSource,
    pub effect_kind: crate::LiveBridgeEffectKind,
    pub physical_outcome: crate::LiveBridgeEffectOutcome,
    pub failure_kind: ToolDispatchTerminalErrorKind,
}

/// The canonical, typed terminal cause for a runtime/tool-dispatch failure.
///
/// This is the single owner of the terminal failure truth: it carries the
/// typed [`ToolError`] itself, and both the coarse classification
/// ([`Self::kind`]) and the model-facing transcript text
/// ([`Self::to_transcript_content`]) are derived purely from that one value.
/// Callers must not re-derive the failure class by parsing transcript text.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolDispatchTerminalCause {
    RuntimeToolError { error: ToolError },
}

impl ToolDispatchTerminalCause {
    #[must_use]
    pub fn runtime_tool_error(error: &ToolError) -> Self {
        Self::RuntimeToolError {
            error: error.clone(),
        }
    }

    /// The coarse terminal error classification, derived from the typed error.
    #[must_use]
    pub fn kind(&self) -> ToolDispatchTerminalErrorKind {
        match self {
            Self::RuntimeToolError { error } => ToolDispatchTerminalErrorKind::from(error),
        }
    }

    /// The canonical model-facing transcript text for this terminal cause.
    ///
    /// This is the sole render boundary: the transcript JSON is derived purely
    /// from the typed error, so the transcript is never a second source of
    /// truth that can drift from the typed cause.
    #[must_use]
    pub fn to_transcript_content(&self) -> String {
        match self {
            Self::RuntimeToolError { error } => error.to_transcript_content(),
        }
    }

    #[must_use]
    pub fn is_runtime_tool_timeout(&self) -> bool {
        self.kind() == ToolDispatchTerminalErrorKind::Timeout
    }
}

#[derive(Debug, Clone)]
pub struct ToolDispatchOutcome {
    /// The tool result for the conversation transcript.
    pub result: crate::types::ToolResult,
    /// Async operations started by this dispatch, with typed wait policies.
    ///
    /// Empty for synchronous tools. Barrier ops block the turn boundary;
    /// detached ops run independently.
    pub async_ops: Vec<AsyncOpRef>,
    /// Session-level effects to be merged by the turn owner after the batch.
    ///
    /// Most tools return an empty vec. Tools that need durable session state
    /// changes (e.g., mob authority grants) emit typed effects here instead
    /// of calling `SessionService` from inside dispatch.
    pub session_effects: Vec<SessionEffect>,
    /// Runtime/tool-dispatch-authored terminal cause.
    ///
    /// Tool-authored `is_error` results intentionally leave this empty; callers
    /// must not infer canonical timeout/failure classes by parsing result text.
    terminal_cause: Option<ToolDispatchTerminalCause>,
    /// The deferred tools a `tool_catalog_load` call accepted, and how far
    /// the load has got. Read it through [`Self::deferred_tool_load`].
    deferred_tool_load: Option<DeferredToolLoad>,
}

/// How far a `tool_catalog_load` call has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeferredToolLoadStage {
    /// The catalog accepted the names. A newly requested tool waits for the
    /// session's next boundary.
    Requested,
    /// Every accepted name is callable for the session: the owner of the
    /// boundary after an external call committed the request.
    LoadedForSession,
}

/// The deferred tools a `tool_catalog_load` call accepted (newly requested,
/// or requested or loaded before), with the stage the load has reached.
///
/// It is typed data for whoever owns the boundary after the call. The agent
/// loop applies a request at its next model-call boundary. An external call
/// (a live conversation's tool call) has no such boundary, so the agent
/// commits the request before the call returns and reports
/// [`DeferredToolLoadStage::LoadedForSession`]; a live host then updates its
/// provider's tool list before it hands the result to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredToolLoad {
    names: Vec<crate::types::ToolName>,
    stage: DeferredToolLoadStage,
    refused: Vec<DeferredToolLoadRefusal>,
}

/// Why a tool that a `tool_catalog_load` call accepted cannot be called
/// where the load was made.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeferredToolLoadRefusalReason {
    /// The session's active tool filter hides the tool. The request stays
    /// recorded; the tool is not callable.
    HiddenBySessionFilter,
    /// The provider of the conversation that made the load would refuse the
    /// tool's definition, so the tool was withheld from that conversation's
    /// tool list. It is loaded for the session.
    RefusedByConversationProvider {
        /// The provider-side reason, as its adapter's local check gave it.
        detail: String,
    },
}

impl DeferredToolLoadRefusalReason {
    /// What the refusal means for the tool, as the result states it.
    #[must_use]
    pub fn availability(&self) -> crate::tool_catalog::DeferredToolLoadAvailability {
        match self {
            Self::HiddenBySessionFilter => {
                crate::tool_catalog::DeferredToolLoadAvailability::NotLoaded
            }
            Self::RefusedByConversationProvider { .. } => {
                crate::tool_catalog::DeferredToolLoadAvailability::LoadedNotInThisConversation
            }
        }
    }

    /// One plain sentence for the model.
    #[must_use]
    pub fn note(&self) -> String {
        match self {
            Self::HiddenBySessionFilter => "The session's tool filter hides this tool. It becomes \
                                            callable at the session's next model call if the \
                                            filter allows it."
                .to_string(),
            Self::RefusedByConversationProvider { detail } => format!(
                "This conversation's provider cannot take this tool's definition, so the tool \
                 was left out of this conversation: {detail}"
            ),
        }
    }
}

/// One tool of a load that cannot be called where the load was made, with
/// the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredToolLoadRefusal {
    name: crate::types::ToolName,
    reason: DeferredToolLoadRefusalReason,
}

impl DeferredToolLoadRefusal {
    #[must_use]
    pub fn name(&self) -> &crate::types::ToolName {
        &self.name
    }

    #[must_use]
    pub fn reason(&self) -> &DeferredToolLoadRefusalReason {
        &self.reason
    }
}

impl DeferredToolLoad {
    /// The load a catalog control tool reports for the names it accepted.
    #[must_use]
    pub fn requested(names: impl IntoIterator<Item = crate::types::ToolName>) -> Self {
        Self {
            names: names.into_iter().collect(),
            stage: DeferredToolLoadStage::Requested,
            refused: Vec::new(),
        }
    }

    /// The accepted names that are not refused.
    #[must_use]
    pub fn names(&self) -> &[crate::types::ToolName] {
        &self.names
    }

    #[must_use]
    pub fn stage(&self) -> DeferredToolLoadStage {
        self.stage
    }

    /// The accepted names that cannot be called where the load was made,
    /// each with its reason.
    #[must_use]
    pub fn refused(&self) -> &[DeferredToolLoadRefusal] {
        &self.refused
    }

    /// The same load with `name` moved from the accepted names to the
    /// refused ones. A name the load does not hold changes nothing.
    #[must_use]
    pub fn refuse(mut self, name: &str, reason: DeferredToolLoadRefusalReason) -> Self {
        if let Some(position) = self.names.iter().position(|held| held.as_str() == name) {
            let name = self.names.remove(position);
            self.refused.push(DeferredToolLoadRefusal { name, reason });
        }
        self
    }

    /// The same load once its names are callable for the session. The owner
    /// of the boundary after an external call reports this, after it
    /// committed the request and found every name it keeps visible. A
    /// report made by anyone else goes through that boundary again.
    #[must_use]
    pub fn loaded_for_session(mut self) -> Self {
        self.stage = DeferredToolLoadStage::LoadedForSession;
        self
    }
}

/// Optional timeout policy supplied by an external tool-dispatch caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDispatchTimeoutPolicy {
    /// Use the caller's default timeout value.
    Default { timeout: std::time::Duration },
    /// Do not apply a caller-specific timeout. The dispatcher may still apply
    /// its own normal execution policy.
    Disabled,
    /// Apply this finite caller-specific timeout.
    Finite { timeout: std::time::Duration },
}

impl ToolDispatchTimeoutPolicy {
    #[must_use]
    pub fn timeout(self) -> Option<std::time::Duration> {
        match self {
            Self::Default { timeout } | Self::Finite { timeout } => Some(timeout),
            Self::Disabled => None,
        }
    }

    #[must_use]
    pub fn timeout_ms(self) -> Option<u64> {
        self.timeout()
            .map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX))
    }
}

/// One `tool_availability` entry of a load's result body.
fn tool_availability_entry(
    name: &str,
    availability: crate::tool_catalog::DeferredToolLoadAvailability,
    reason: Option<String>,
) -> serde_json::Value {
    let mut entry = serde_json::Map::new();
    entry.insert(
        "name".to_string(),
        serde_json::Value::String(name.to_string()),
    );
    entry.insert(
        "availability".to_string(),
        serde_json::Value::String(availability.as_str().to_string()),
    );
    if let Some(reason) = reason {
        entry.insert("reason".to_string(), serde_json::Value::String(reason));
    }
    serde_json::Value::Object(entry)
}

impl ToolDispatchOutcome {
    /// Create an outcome with explicit async operations and session effects.
    pub fn new(
        result: crate::types::ToolResult,
        async_ops: Vec<AsyncOpRef>,
        session_effects: Vec<SessionEffect>,
    ) -> Self {
        Self {
            result,
            async_ops,
            session_effects,
            terminal_cause: None,
            deferred_tool_load: None,
        }
    }

    /// Create an outcome with no async operations or session effects (synchronous tool).
    pub fn sync_result(result: crate::types::ToolResult) -> Self {
        Self::new(result, Vec::new(), Vec::new())
    }

    /// The deferred tools this call's `tool_catalog_load` accepted, when it
    /// accepted any, and the stage the load has reached.
    ///
    /// A dispatcher that only emits [`SessionEffect::RequestDeferredTools`]
    /// reports the names of that effect as requested.
    #[must_use]
    pub fn deferred_tool_load(&self) -> Option<DeferredToolLoad> {
        if let Some(load) = self.deferred_tool_load.as_ref() {
            return Some(load.clone());
        }
        let names = self
            .session_effects
            .iter()
            .filter_map(|effect| match effect {
                SessionEffect::RequestDeferredTools { authorities } => Some(authorities),
                _ => None,
            })
            .flatten()
            .map(|authority| authority.name.clone())
            .collect::<Vec<_>>();
        (!names.is_empty()).then(|| DeferredToolLoad::requested(names))
    }

    /// Report the deferred tools this call's `tool_catalog_load` accepted.
    #[must_use]
    pub fn with_deferred_tool_load(mut self, load: DeferredToolLoad) -> Self {
        self.deferred_tool_load = Some(load);
        self
    }

    /// State in the result, per tool, whether the tools a committed load
    /// made callable for the session can be called in the conversation that
    /// loaded them.
    ///
    /// `availability` is the caller's fact about the tools the load still
    /// holds; the tools the load refuses carry their own. The result's JSON
    /// body gets `tool_availability` (one entry per tool: `name`,
    /// `availability`, and a `reason` for a refused one), the summary field
    /// `availability` (the one value when every tool has the same, `mixed`
    /// otherwise) and one plain sentence beside it, so the model is told
    /// what is true instead of a bare acceptance. The caller that knows the
    /// conversation (a live host, after it updated or failed to update its
    /// provider's tool list) states the fact; the wording is owned here.
    #[must_use]
    pub fn with_deferred_load_availability(
        mut self,
        availability: crate::tool_catalog::DeferredToolLoadAvailability,
    ) -> Self {
        use crate::tool_catalog::DeferredToolLoadAvailability;
        let per_tool: Vec<(String, DeferredToolLoadAvailability, Option<String>)> = self
            .deferred_tool_load
            .iter()
            .flat_map(|load| {
                let held = load
                    .names()
                    .iter()
                    .map(|name| (name.to_string(), availability, None));
                let refused = load.refused().iter().map(|refusal| {
                    (
                        refusal.name().to_string(),
                        refusal.reason().availability(),
                        Some(refusal.reason().note()),
                    )
                });
                held.chain(refused)
            })
            .collect();
        // One value when every tool has the same (or the outcome names no
        // tool), `mixed` otherwise.
        let every_tool_has = match per_tool.first() {
            None => Some(availability),
            Some((_, first, _)) => per_tool
                .iter()
                .all(|(_, other, _)| other == first)
                .then_some(*first),
        };
        let (summary, note) = match every_tool_has {
            Some(same) => (same.as_str(), same.note()),
            None => (
                crate::tool_catalog::DEFERRED_TOOL_LOAD_AVAILABILITY_MIXED,
                crate::tool_catalog::DEFERRED_TOOL_LOAD_AVAILABILITY_MIXED_NOTE,
            ),
        };
        let entries = per_tool
            .into_iter()
            .map(|(name, availability, reason)| {
                tool_availability_entry(&name, availability, reason)
            })
            .collect::<Vec<_>>();
        let mut body = self.deferred_load_result_body();
        body.insert(
            "availability".to_string(),
            serde_json::Value::String(summary.to_string()),
        );
        body.insert(
            "availability_note".to_string(),
            serde_json::Value::String(note.to_string()),
        );
        if entries.is_empty() {
            body.remove("tool_availability");
        } else {
            body.insert(
                "tool_availability".to_string(),
                serde_json::Value::Array(entries),
            );
        }
        self.result
            .set_text_content(serde_json::Value::Object(body).to_string());
        self
    }

    /// Name in the result the tools this load refuses, each with its reason.
    /// The boundary that refuses them calls this, so the result is truthful
    /// even when no host states the conversation's side afterwards (a host
    /// that does replaces the list with the whole one).
    #[must_use]
    pub(crate) fn with_deferred_load_refusals_stated(mut self) -> Self {
        let entries = self
            .deferred_tool_load
            .iter()
            .flat_map(|load| {
                load.refused().iter().map(|refusal| {
                    tool_availability_entry(
                        refusal.name().as_str(),
                        refusal.reason().availability(),
                        Some(refusal.reason().note()),
                    )
                })
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return self;
        }
        let mut body = self.deferred_load_result_body();
        body.insert(
            "tool_availability".to_string(),
            serde_json::Value::Array(entries),
        );
        self.result
            .set_text_content(serde_json::Value::Object(body).to_string());
        self
    }

    /// The result's JSON body, or the result's text under `result` when it
    /// is not a JSON object.
    fn deferred_load_result_body(&self) -> serde_json::Map<String, serde_json::Value> {
        let text = self.result.text_content();
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(body)) => body,
            _ => {
                let mut body = serde_json::Map::new();
                body.insert("result".to_string(), serde_json::Value::String(text));
                body
            }
        }
    }

    #[must_use]
    pub fn terminal_cause(&self) -> Option<&ToolDispatchTerminalCause> {
        self.terminal_cause.as_ref()
    }

    #[must_use]
    pub fn settlement_failures(&self) -> &[ToolDispatchSettlementFailure] {
        &self.result.settlement_failures
    }

    pub(crate) fn extend_settlement_failures(
        &mut self,
        failures: impl IntoIterator<Item = ToolDispatchSettlementFailure>,
    ) {
        self.result.settlement_failures.extend(failures);
    }

    #[must_use]
    pub fn is_runtime_tool_timeout(&self) -> bool {
        self.terminal_cause
            .as_ref()
            .is_some_and(ToolDispatchTerminalCause::is_runtime_tool_timeout)
    }

    pub(crate) fn clear_terminal_cause(&mut self) {
        self.terminal_cause = None;
    }
}

impl From<crate::types::ToolResult> for ToolDispatchOutcome {
    fn from(result: crate::types::ToolResult) -> Self {
        Self::sync_result(result)
    }
}

/// Convert a denied/failed tool dispatch into the canonical terminal tool
/// outcome shape used by both model-driven and external tool calls.
pub fn terminal_tool_outcome_for_error(
    tool_use_id: impl Into<String>,
    error: ToolError,
) -> ToolDispatchOutcome {
    let (error, settlement_failures) = error.into_primary_and_settlement_failures();
    let terminal_cause = ToolDispatchTerminalCause::RuntimeToolError { error };
    // The transcript text is derived purely from the typed terminal cause, so
    // the cause is the sole source of truth and the transcript can never drift
    // from it. Rendering is infallible (no fallback string launder).
    let content = terminal_cause.to_transcript_content();
    let mut outcome = ToolDispatchOutcome::sync_result(crate::types::ToolResult::new(
        tool_use_id.into(),
        content,
        true,
    ));
    outcome.terminal_cause = Some(terminal_cause);
    outcome.result.settlement_failures = settlement_failures;
    outcome
}

impl OperationId {
    /// Create a new operation ID
    pub fn new() -> Self {
        Self(crate::time_compat::new_uuid_v7())
    }

    /// Derive the stable MeerkatMachine operation that owns one session's
    /// explicit wait binding to a realm-qualified detached job.
    ///
    /// Determinism lets a reconstructed runtime re-register the same binding
    /// after volatile non-terminal operation state is discarded. It does not
    /// claim, retry, or otherwise mutate detached-job execution authority.
    pub fn for_detached_job_wait(
        session_id: &crate::types::SessionId,
        realm_id: &str,
        job_id: &str,
    ) -> Self {
        let name = format!(
            "meerkat.detached_job_wait.v1:{}:{}:{}:{}:{}",
            session_id,
            realm_id.len(),
            realm_id,
            job_id.len(),
            job_id
        );
        Self(Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes()))
    }
}

impl Default for OperationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for OperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What kind of work the operation performs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    /// MCP or internal tool call
    ToolCall,
    /// Shell command execution
    ShellCommand,
}

/// Shape of the operation's result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultShape {
    /// Single result value
    Single,
    /// Streaming output (progress events)
    Stream,
    /// Multiple results (e.g., fork branches)
    Batch,
}

/// How much context a delegated branch receives
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ContextStrategy {
    /// Complete conversation history (Fork default)
    #[default]
    FullHistory,
    /// Last N turns from parent
    LastTurns(u32),
    /// Compressed summary of conversation
    Summary { max_tokens: u32 },
    /// Explicit message list
    Custom { messages: Vec<Message> },
}

/// How to allocate budget when forking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ForkBudgetPolicy {
    /// Split remaining budget equally among branches
    #[default]
    Equal,
    /// Split proportionally based on weights
    Proportional,
    /// Fixed budget per branch
    Fixed(u64),
    /// Give all remaining budget to each branch
    Remaining,
}

/// Tool access control for delegated branches
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ToolAccessConstraint {
    AllowNames(ToolNameSet),
    DenyNames(ToolNameSet),
    ReadOnly,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ToolAccessPolicy {
    /// Inherit all tools from parent
    #[default]
    Inherit,
    /// Only allow specific tools
    AllowList(ToolNameSet),
    /// Block specific tools
    DenyList(ToolNameSet),
    /// Only tools whose owning dispatcher declares
    /// [`crate::ToolMutationClass::ReadOnly`] may execute.
    ///
    /// Name-independent intent: a tool the dispatcher chain does not declare
    /// read-only is denied, so tools that appear later (MCP servers finishing
    /// their handshake, host bundles) never widen the policy. See
    /// [`crate::tool_execution_policy`] for the exact guarantee boundary this
    /// declaration does and does not cover.
    ///
    /// Downgrade consequence: this enum is closed and adjacently tagged, and
    /// it is persisted in durable session metadata
    /// (`SessionMetadata.tooling.tool_access_policy`), so a binary older than
    /// the release that added this variant cannot decode the metadata of a
    /// session launched with read-only intent. Failing to decode is the
    /// intended outcome (a downgrade that silently read it as `Inherit` would
    /// drop the enforcement), but the session is unresumable on that binary
    /// until it is upgraded.
    ReadOnly,
    /// Every contained constraint must admit the call.
    Constraints(Vec<ToolAccessConstraint>),
}

/// A tool restriction declared by whoever configured the agent (a mob
/// profile's `tools.read_only` and `tools.deny`), as opposed to the per-launch
/// [`ToolAccessPolicy`] a host or parent grants.
///
/// The factory conjoins it with the launch policy for the execution gate on
/// every build, but persists only the launch part as the session's spawn
/// policy, so each build (including a resume) recomputes the declaration from
/// the current configuration: a removed deny entry or a lifted read-only
/// declaration takes effect, and a launch narrowing survives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredToolRestriction {
    /// Who declared it (for example the mob profile name), for errors.
    pub declared_by: String,
    /// The tool families the declaring configuration enabled, for errors.
    pub enabled_families: Vec<String>,
    /// Only tools their owning dispatcher declares read-only may execute.
    pub read_only: bool,
    /// Tool names that may not execute. Each must be a tool the build
    /// composed or a name in a tool vocabulary: the factory's built-in family
    /// vocabularies and [`Self::vocabulary`]. A known name the build did not
    /// mount is inert; any other name fails the build as `DeclaredToolUnknown`.
    /// The execution gate matches by name, so a denied name refuses whichever
    /// mounted tool carries it, and the same name leaves the session's visible
    /// tool scope ([`crate::ToolExecutionPolicy::static_visibility_filter`]).
    pub deny: ToolNameSet,
    /// The tool names the declaring configuration owns, by source: the mob
    /// operator and agent mob tools, the exposed names of the MCP servers it
    /// declares, and its tool bundles. A bundle the configuration opted into
    /// but this build withheld is listed as
    /// [`ToolVocabularySource::WithheldBundle`] with no names: its tools
    /// cannot be known, so while one is listed a deny name in no vocabulary
    /// is deferred to the execution gate, as for
    /// [`Self::deferred_mcp_servers`].
    pub vocabulary: std::collections::BTreeMap<ToolVocabularySource, ToolNameSet>,
    /// Declared MCP servers that map no tool names, so their tools cannot be
    /// known before they connect. While any is declared, a deny name in no
    /// vocabulary is not a build error: its validation is deferred and the
    /// execution gate enforces it by name.
    pub deferred_mcp_servers: std::collections::BTreeSet<String>,
}

/// Where a tool name a deny list may declare comes from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ToolVocabularySource {
    /// A built-in tool family (`shell`, `memory`, ...), named by the factory.
    Family(String),
    /// The mob operator tools a mob member mounts.
    MobOperator,
    /// The agent-facing mob tools.
    AgentMob,
    /// The exposed tool names a declared MCP server config maps.
    McpServer(String),
    /// A host tool bundle.
    Bundle(String),
    /// A host tool bundle the configuration opted into that this build
    /// withheld, because the host does not offer it to this build. It names
    /// no tools.
    WithheldBundle(String),
}

impl std::fmt::Display for ToolVocabularySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Family(family) => write!(f, "{family} tools"),
            Self::MobOperator => f.write_str("mob operator tools"),
            Self::AgentMob => f.write_str("agent mob tools"),
            Self::McpServer(server) => write!(f, "MCP server '{server}'"),
            Self::Bundle(bundle) => write!(f, "tool bundle '{bundle}'"),
            Self::WithheldBundle(bundle) => write!(f, "withheld tool bundle '{bundle}'"),
        }
    }
}

impl DeclaredToolRestriction {
    /// Whether the declaration restricts nothing.
    pub fn is_unrestricted(&self) -> bool {
        !self.read_only && self.deny.is_empty()
    }

    /// The access policy this declaration imposes, or `None` when it imposes
    /// none.
    pub fn policy(&self) -> Result<Option<ToolAccessPolicy>, crate::ToolExecutionPolicyError> {
        let deny = (!self.deny.is_empty()).then(|| ToolAccessPolicy::DenyList(self.deny.clone()));
        match (deny, self.read_only) {
            (None, false) => Ok(None),
            (Some(deny), false) => Ok(Some(deny)),
            (None, true) => Ok(Some(ToolAccessPolicy::ReadOnly)),
            (Some(deny), true) => deny.conjoin(ToolAccessPolicy::ReadOnly).map(Some),
        }
    }

    /// Conjoin this declaration with a launch policy into the effective
    /// policy the execution gate enforces.
    pub fn conjoin_with_launch_policy(
        &self,
        launch: Option<ToolAccessPolicy>,
    ) -> Result<Option<ToolAccessPolicy>, crate::ToolExecutionPolicyError> {
        Ok(match (launch, self.policy()?) {
            (launch, None) => launch,
            (None, declared) => declared,
            (Some(launch), Some(declared)) => Some(launch.conjoin(declared)?),
        })
    }
}

/// The launch (spawn-site) part of a session's tool access policy, recorded
/// beside the effective policy so a resume can recompute the session's
/// [`DeclaredToolRestriction`] from the current configuration instead of
/// restoring an old one.
///
/// A typed record rather than `Option<ToolAccessPolicy>`: an unrestricted
/// launch must stay distinguishable from a session persisted before the
/// launch part was recorded at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpawnToolAccessPolicy {
    /// The launch granted no restriction of its own.
    Unrestricted,
    /// The launch granted this resolved policy.
    Restricted { policy: ToolAccessPolicy },
}

impl SpawnToolAccessPolicy {
    /// Record a resolved launch policy (`None` is unrestricted).
    pub fn from_launch(policy: Option<ToolAccessPolicy>) -> Self {
        match policy {
            None => Self::Unrestricted,
            Some(policy) => Self::Restricted { policy },
        }
    }

    /// The recorded launch policy (`None` is unrestricted).
    pub fn into_launch(self) -> Option<ToolAccessPolicy> {
        match self {
            Self::Unrestricted => None,
            Self::Restricted { policy } => Some(policy),
        }
    }
}

impl ToolAccessPolicy {
    /// Conjoin two already resolved policies without discarding either ceiling.
    pub fn conjoin(self, other: Self) -> Result<Self, crate::ToolExecutionPolicyError> {
        fn append(
            policy: ToolAccessPolicy,
            constraints: &mut Vec<ToolAccessConstraint>,
        ) -> Result<(), crate::ToolExecutionPolicyError> {
            match policy {
                ToolAccessPolicy::Inherit => {
                    Err(crate::ToolExecutionPolicyError::UnresolvedInherit)
                }
                ToolAccessPolicy::AllowList(names) => {
                    constraints.push(ToolAccessConstraint::AllowNames(names));
                    Ok(())
                }
                ToolAccessPolicy::DenyList(names) => {
                    constraints.push(ToolAccessConstraint::DenyNames(names));
                    Ok(())
                }
                ToolAccessPolicy::ReadOnly => {
                    constraints.push(ToolAccessConstraint::ReadOnly);
                    Ok(())
                }
                ToolAccessPolicy::Constraints(nested) => {
                    if nested.is_empty() {
                        return Err(crate::ToolExecutionPolicyError::EmptyConstraints);
                    }
                    constraints.extend(nested);
                    Ok(())
                }
            }
        }

        let mut constraints = Vec::new();
        append(self, &mut constraints)?;
        append(other, &mut constraints)?;
        Ok(Self::Constraints(constraints))
    }
}

/// Policy for operation execution
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperationPolicy {
    /// Timeout for this operation
    pub timeout_ms: Option<u64>,
    /// Whether to cancel on parent cancellation
    pub cancel_on_parent_cancel: bool,
    /// Whether to include in checkpoints
    pub checkpoint_results: bool,
}

/// Complete operation specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationSpec {
    pub id: OperationId,
    pub kind: WorkKind,
    pub result_shape: ResultShape,
    pub policy: OperationPolicy,
    pub budget_reservation: BudgetLimits,
    pub depth: u32,
    pub depends_on: Vec<OperationId>,
    pub context: Option<ContextStrategy>,
    pub tool_access: Option<ToolAccessPolicy>,
}

/// Result of a completed operation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationResult {
    pub id: OperationId,
    pub content: String,
    pub is_error: bool,
    pub duration_ms: u64,
    pub tokens_used: u64,
}

/// Events from operations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpEvent {
    /// Operation started executing
    Started { id: OperationId, kind: WorkKind },

    /// Progress update (for streaming operations)
    Progress {
        id: OperationId,
        message: String,
        percent: Option<f32>,
    },

    /// Operation completed successfully
    Completed {
        id: OperationId,
        result: OperationResult,
    },

    /// Operation failed
    Failed { id: OperationId, error: String },

    /// Operation was cancelled
    Cancelled { id: OperationId },
}

/// Concurrency limits for operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcurrencyLimits {
    /// Maximum delegated-branch nesting depth
    pub max_depth: u32,
    /// Maximum concurrent operations (all types)
    pub max_concurrent_ops: usize,
    /// Maximum concurrent delegated branches specifically
    pub max_concurrent_agents: usize,
    /// Maximum children per parent agent
    pub max_children_per_agent: usize,
}

impl Default for ConcurrencyLimits {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_concurrent_ops: 32,
            max_concurrent_agents: 8,
            max_children_per_agent: 5,
        }
    }
}

/// Specification for spawning a new delegated branch
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpawnSpec {
    /// The prompt/task for the delegated branch
    pub prompt: String,
    /// How much context the delegated branch receives
    pub context: ContextStrategy,
    /// Which tools the delegated branch can access
    pub tool_access: ToolAccessPolicy,
    /// Budget allocation for the delegated branch
    pub budget: BudgetLimits,
    /// If false, the delegated branch cannot spawn/fork further
    pub allow_spawn: bool,
    /// System prompt override (None = inherit from parent)
    pub system_prompt: Option<String>,
}

/// A branch in a fork operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkBranch {
    /// Identifier for this branch
    pub name: String,
    /// The prompt/task for this branch
    pub prompt: String,
    /// Tool access override (None = inherit)
    pub tool_access: Option<ToolAccessPolicy>,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn spawn_tool_access_policy_keeps_unrestricted_distinct_from_unrecorded() {
        let unrestricted = SpawnToolAccessPolicy::from_launch(None);
        assert_eq!(
            serde_json::to_value(&unrestricted).unwrap(),
            serde_json::json!({ "kind": "unrestricted" })
        );
        let restricted = SpawnToolAccessPolicy::from_launch(Some(ToolAccessPolicy::DenyList(
            ["shell"].into_iter().collect(),
        )));
        let decoded: SpawnToolAccessPolicy =
            serde_json::from_value(serde_json::to_value(&restricted).unwrap()).unwrap();
        assert_eq!(decoded, restricted);
        assert_eq!(unrestricted.into_launch(), None);
        assert_eq!(
            decoded.into_launch(),
            Some(ToolAccessPolicy::DenyList(["shell"].into_iter().collect()))
        );
    }

    #[test]
    fn declared_tool_restriction_composes_deny_read_only_and_launch() {
        let restriction = DeclaredToolRestriction {
            declared_by: "profile 'peer'".to_string(),
            enabled_families: vec!["mob".to_string()],
            read_only: false,
            deny: ["mob_wire"].into_iter().collect(),
            vocabulary: std::collections::BTreeMap::new(),
            deferred_mcp_servers: std::collections::BTreeSet::new(),
        };
        assert!(!restriction.is_unrestricted());
        assert_eq!(
            restriction.policy().unwrap(),
            Some(ToolAccessPolicy::DenyList(
                ["mob_wire"].into_iter().collect()
            ))
        );
        let launch = ToolAccessPolicy::AllowList(["mob_wire", "mob_list"].into_iter().collect());
        let effective = crate::ToolExecutionPolicy::resolve(
            restriction
                .conjoin_with_launch_policy(Some(launch))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(effective.permits_call("mob_list", crate::ToolMutationClass::Mutating));
        assert!(!effective.permits_call("mob_wire", crate::ToolMutationClass::Mutating));
        assert!(!effective.permits_call("other", crate::ToolMutationClass::ReadOnly));

        let open = DeclaredToolRestriction {
            deny: ToolNameSet::new(),
            ..restriction
        };
        assert!(open.is_unrestricted());
        assert_eq!(open.conjoin_with_launch_policy(None).unwrap(), None);
    }

    fn generated_mob_authority_for_test() -> crate::service::MobToolAuthorityContext {
        crate::service::MobToolAuthorityContext::generated_for_test(
            crate::service::OpaquePrincipalToken::new("generated-effect-test"),
            true,
            true,
            false,
            std::collections::BTreeSet::from(["test-mob".to_string()]),
            std::collections::BTreeMap::new(),
            None,
            None,
        )
    }

    #[test]
    fn barrier_constructor_produces_barrier_policy() {
        assert_eq!(WaitPolicy::barrier(), WaitPolicy::Barrier);
        let op_ref = AsyncOpRef::barrier(OperationId::new());
        assert_eq!(op_ref.wait_policy, WaitPolicy::Barrier);
    }

    #[test]
    fn detached_constructor_produces_detached_policy() {
        assert_eq!(WaitPolicy::detached(), WaitPolicy::Detached);
        let op_ref = AsyncOpRef::detached(OperationId::new());
        assert_eq!(op_ref.wait_policy, WaitPolicy::Detached);
    }

    #[test]
    fn test_operation_id_encoding() {
        let id = OperationId::new();
        let json = serde_json::to_string(&id).unwrap();

        let parsed: OperationId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_work_kind_serialization() {
        assert_eq!(
            serde_json::to_value(WorkKind::ToolCall).unwrap(),
            "tool_call"
        );
        assert_eq!(
            serde_json::to_value(WorkKind::ShellCommand).unwrap(),
            "shell_command"
        );
    }

    #[test]
    fn test_context_strategy_serialization() {
        let full = ContextStrategy::FullHistory;
        let json = serde_json::to_value(&full).unwrap();
        assert_eq!(json["type"], "full_history");

        let last = ContextStrategy::LastTurns(5);
        let json = serde_json::to_value(&last).unwrap();
        assert_eq!(json["type"], "last_turns");
        // Adjacently-tagged: {"type": "last_turns", "value": 5}
        assert_eq!(json["value"], 5);

        let summary = ContextStrategy::Summary { max_tokens: 1000 };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["type"], "summary");
        // Adjacently-tagged struct variant: {"type": "summary", "value": {"max_tokens": 1000}}
        assert_eq!(json["value"]["max_tokens"], 1000);

        // Roundtrip
        let parsed: ContextStrategy = serde_json::from_value(json).unwrap();
        match parsed {
            ContextStrategy::Summary { max_tokens } => assert_eq!(max_tokens, 1000),
            _ => unreachable!("Wrong variant"),
        }
    }

    #[test]
    fn test_fork_budget_policy_serialization() {
        let policies = vec![
            (ForkBudgetPolicy::Equal, "equal"),
            (ForkBudgetPolicy::Proportional, "proportional"),
            (ForkBudgetPolicy::Remaining, "remaining"),
        ];

        for (policy, expected_type) in policies {
            let json = serde_json::to_value(&policy).unwrap();
            assert_eq!(json["type"], expected_type);
        }

        let fixed = ForkBudgetPolicy::Fixed(5000);
        let json = serde_json::to_value(&fixed).unwrap();
        assert_eq!(json["type"], "fixed");
        // Adjacently-tagged: {"type": "fixed", "value": 5000}
        assert_eq!(json["value"], 5000);

        // Roundtrip
        let parsed: ForkBudgetPolicy = serde_json::from_value(json).unwrap();
        match parsed {
            ForkBudgetPolicy::Fixed(tokens) => assert_eq!(tokens, 5000),
            _ => unreachable!("Wrong variant"),
        }
    }

    #[test]
    fn test_tool_access_policy_serialization() {
        let inherit = ToolAccessPolicy::Inherit;
        let json = serde_json::to_value(&inherit).unwrap();
        assert_eq!(json["type"], "inherit");

        let allow = ToolAccessPolicy::AllowList(["read_file", "write_file"].into_iter().collect());
        let json = serde_json::to_value(&allow).unwrap();
        assert_eq!(json["type"], "allow_list");
        // Adjacently-tagged: {"type": "allow_list", "value": [...]}
        assert!(json["value"].is_array());

        let deny = ToolAccessPolicy::DenyList(["dangerous_tool"].into_iter().collect());
        let json = serde_json::to_value(&deny).unwrap();
        assert_eq!(json["type"], "deny_list");
        assert!(json["value"].is_array());

        // Roundtrip
        let parsed: ToolAccessPolicy = serde_json::from_value(json).unwrap();
        match parsed {
            ToolAccessPolicy::DenyList(tools) => {
                assert_eq!(tools.len(), 1);
                assert!(tools.contains("dangerous_tool"));
            }
            _ => unreachable!("Wrong variant"),
        }
    }

    #[test]
    fn test_op_event_serialization() {
        let events = vec![
            OpEvent::Started {
                id: OperationId::new(),
                kind: WorkKind::ToolCall,
            },
            OpEvent::Progress {
                id: OperationId::new(),
                message: "50% complete".to_string(),
                percent: Some(0.5),
            },
            OpEvent::Completed {
                id: OperationId::new(),
                result: OperationResult {
                    id: OperationId::new(),
                    content: "result".to_string(),
                    is_error: false,
                    duration_ms: 100,
                    tokens_used: 50,
                },
            },
            OpEvent::Failed {
                id: OperationId::new(),
                error: "timeout".to_string(),
            },
            OpEvent::Cancelled {
                id: OperationId::new(),
            },
        ];

        for event in events {
            let json = serde_json::to_value(&event).unwrap();
            assert!(json.get("type").is_some());

            // Roundtrip
            let _: OpEvent = serde_json::from_value(json).unwrap();
        }
    }

    #[test]
    fn test_concurrency_limits_default() {
        let limits = ConcurrencyLimits::default();
        assert_eq!(limits.max_depth, 3);
        assert_eq!(limits.max_concurrent_ops, 32);
        assert_eq!(limits.max_concurrent_agents, 8);
        assert_eq!(limits.max_children_per_agent, 5);
    }

    #[test]
    fn session_effect_replace_mob_authority_context_deserializes_without_authority_seal() {
        let authority_context = generated_mob_authority_for_test();
        let incarnation = authority_context.mob_incarnations()["test-mob"];
        let effect = SessionEffect::ReplaceMobToolAuthorityContext { authority_context };
        let json = serde_json::to_value(&effect).unwrap();
        let parsed: SessionEffect = serde_json::from_value(json).unwrap();
        match parsed {
            SessionEffect::ReplaceMobToolAuthorityContext { authority_context } => {
                assert!(!authority_context.is_generated_authority_context());
                assert!(!authority_context.can_create_mobs());
                assert!(!authority_context.can_mutate_profiles());
                assert!(!authority_context.can_manage_mob("test-mob", incarnation));
            }
            other => panic!("unexpected session effect: {other:?}"),
        }
    }

    #[test]
    fn tool_dispatch_outcome_with_session_effects() {
        let result = crate::types::ToolResult::new("t1".into(), "ok".into(), false);
        let outcome = ToolDispatchOutcome::new(
            result,
            vec![],
            vec![SessionEffect::ReplaceMobToolAuthorityContext {
                authority_context: generated_mob_authority_for_test(),
            }],
        );
        assert_eq!(outcome.session_effects.len(), 1);
        assert_eq!(outcome.terminal_cause(), None);
    }

    #[test]
    fn tool_dispatch_outcome_sync_result_has_empty_effects() {
        let result = crate::types::ToolResult::new("t1".into(), "ok".into(), false);
        let outcome = ToolDispatchOutcome::sync_result(result);
        assert!(outcome.session_effects.is_empty());
        assert_eq!(outcome.terminal_cause(), None);
    }

    #[test]
    fn terminal_tool_outcome_carries_runtime_timeout_cause() {
        let outcome = terminal_tool_outcome_for_error("t1", ToolError::timeout("slow_tool", 50));

        assert!(outcome.result.is_error);
        assert!(outcome.is_runtime_tool_timeout());
        let cause = outcome.terminal_cause().expect("terminal cause present");
        assert_eq!(cause.kind(), ToolDispatchTerminalErrorKind::Timeout);
        assert_eq!(
            cause,
            &ToolDispatchTerminalCause::RuntimeToolError {
                error: ToolError::timeout("slow_tool", 50),
            }
        );
    }

    #[test]
    fn terminal_tool_outcome_transcript_text_is_derived_purely_from_terminal_cause() {
        // Doctrine (Rule 8): the terminal cause is the sole source of truth.
        // The transcript content must be byte-for-byte derived from it, with
        // no second derivation and no fallback-string launder path.
        let error = ToolError::execution_failed_with_data(
            "boom",
            serde_json::json!({ "detail": "structured", "n": 7 }),
        );
        let outcome = terminal_tool_outcome_for_error("t1", error);

        let cause = outcome.terminal_cause().expect("terminal cause present");
        assert_eq!(outcome.result.text_content(), cause.to_transcript_content());
    }

    #[test]
    fn tool_authored_error_result_has_no_runtime_terminal_cause() {
        let result =
            crate::types::ToolResult::new("t1".into(), "{\"error\":\"timeout\"}".into(), true);
        let outcome = ToolDispatchOutcome::sync_result(result);

        assert!(outcome.result.is_error);
        assert!(!outcome.is_runtime_tool_timeout());
        assert_eq!(outcome.terminal_cause(), None);
    }

    fn deferred_load_authority(name: &str) -> DeferredToolLoadAuthority {
        DeferredToolLoadAuthority::new(
            name,
            crate::session::ToolVisibilityWitness {
                last_seen_provenance: None,
            },
        )
    }

    /// A plain outcome reports no deferred load; one that requests deferred
    /// tools reports their names as requested; an explicit report wins.
    #[test]
    fn deferred_tool_load_is_reported_from_the_request_effect_or_the_explicit_report() {
        let plain = ToolDispatchOutcome::sync_result(crate::types::ToolResult::new(
            "t1".into(),
            "ok".into(),
            false,
        ));
        assert_eq!(plain.deferred_tool_load(), None);

        let mut requesting = plain;
        requesting
            .session_effects
            .push(SessionEffect::RequestDeferredTools {
                authorities: vec![deferred_load_authority("alpha")],
            });
        let load = requesting.deferred_tool_load().expect("a requested load");
        assert_eq!(load.stage(), DeferredToolLoadStage::Requested);
        assert_eq!(load.names(), ["alpha"]);

        let explicit = requesting
            .with_deferred_tool_load(DeferredToolLoad::requested(["alpha".into(), "beta".into()]));
        let load = explicit.deferred_tool_load().expect("the explicit report");
        assert_eq!(load.names(), ["alpha", "beta"]);
        assert_eq!(
            load.loaded_for_session().stage(),
            DeferredToolLoadStage::LoadedForSession
        );
    }

    /// The availability a host states lands in the load's result as a typed
    /// field with one plain sentence, and the rest of the body is kept.
    #[test]
    fn deferred_load_availability_is_stated_in_the_result_body() {
        use crate::tool_catalog::DeferredToolLoadAvailability;
        let body =
            r#"{"accepted_names":["alpha"],"resolutions":[{"name":"alpha","accepted":true}]}"#;
        let outcome = || {
            ToolDispatchOutcome::sync_result(crate::types::ToolResult::new(
                "t1".into(),
                body.into(),
                false,
            ))
        };

        let available: serde_json::Value = serde_json::from_str(
            &outcome()
                .with_deferred_load_availability(DeferredToolLoadAvailability::AvailableNow)
                .result
                .text_content(),
        )
        .expect("a JSON body");
        assert_eq!(available["availability"], "available_now");
        assert_eq!(available["accepted_names"][0], "alpha");
        assert_eq!(available["resolutions"][0]["accepted"], true);

        let not_here = outcome().with_deferred_load_availability(
            DeferredToolLoadAvailability::LoadedNotInThisConversation,
        );
        assert!(
            !not_here.result.is_error,
            "loaded for the session is a result, not an error"
        );
        let not_here: serde_json::Value =
            serde_json::from_str(&not_here.result.text_content()).expect("a JSON body");
        assert_eq!(not_here["availability"], "loaded_not_in_this_conversation");
        assert!(
            not_here["availability_note"]
                .as_str()
                .is_some_and(|note| note.contains("cannot be called in this conversation yet")),
            "the note says the tools are not callable here yet: {not_here}"
        );

        // A body that is not a JSON object is kept whole under `result`.
        let wrapped: serde_json::Value = serde_json::from_str(
            &ToolDispatchOutcome::sync_result(crate::types::ToolResult::new(
                "t1".into(),
                "loaded".into(),
                false,
            ))
            .with_deferred_load_availability(DeferredToolLoadAvailability::AvailableNow)
            .result
            .text_content(),
        )
        .expect("a JSON body");
        assert_eq!(wrapped["result"], "loaded");
        assert_eq!(wrapped["availability"], "available_now");
    }

    /// A load that holds tools and refuses others is stated per tool: every
    /// tool has an entry, a refused one says why, and the summary field is
    /// one value only when every tool has the same.
    #[test]
    fn deferred_load_availability_is_stated_per_tool() {
        use crate::tool_catalog::DeferredToolLoadAvailability;
        let outcome = |load: DeferredToolLoad| {
            ToolDispatchOutcome::sync_result(crate::types::ToolResult::new(
                "t1".into(),
                r#"{"accepted_names":["alpha","beta"]}"#.into(),
                false,
            ))
            .with_deferred_tool_load(load.loaded_for_session())
        };
        let body = |outcome: ToolDispatchOutcome| -> serde_json::Value {
            serde_json::from_str(&outcome.result.text_content()).expect("a JSON body")
        };
        let both = DeferredToolLoad::requested(["alpha".into(), "beta".into()]);

        let all = body(
            outcome(both.clone())
                .with_deferred_load_availability(DeferredToolLoadAvailability::AvailableNow),
        );
        assert_eq!(all["availability"], "available_now");
        assert_eq!(
            all["tool_availability"],
            serde_json::json!([
                { "name": "alpha", "availability": "available_now" },
                { "name": "beta", "availability": "available_now" },
            ])
        );

        let partial = both
            .refuse("beta", DeferredToolLoadRefusalReason::HiddenBySessionFilter)
            // A name the load does not hold changes nothing.
            .refuse(
                "gamma",
                DeferredToolLoadRefusalReason::HiddenBySessionFilter,
            );
        assert_eq!(partial.names(), ["alpha"]);
        assert_eq!(partial.refused().len(), 1);
        assert_eq!(partial.refused()[0].name().as_str(), "beta");

        // The boundary that refuses a tool names it even when no host states
        // the conversation's side.
        let refusals_only = body(outcome(partial.clone()).with_deferred_load_refusals_stated());
        assert_eq!(refusals_only["tool_availability"][0]["name"], "beta");
        assert_eq!(
            refusals_only["tool_availability"][0]["availability"],
            "not_loaded"
        );
        assert!(refusals_only.get("availability").is_none());

        let mixed = body(
            outcome(partial)
                .with_deferred_load_refusals_stated()
                .with_deferred_load_availability(DeferredToolLoadAvailability::AvailableNow),
        );
        assert_eq!(mixed["availability"], "mixed");
        assert_eq!(
            mixed["tool_availability"][0],
            serde_json::json!({ "name": "alpha", "availability": "available_now" })
        );
        assert_eq!(mixed["tool_availability"][1]["name"], "beta");
        assert_eq!(mixed["tool_availability"][1]["availability"], "not_loaded");
        assert!(
            mixed["tool_availability"][1]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("tool filter hides this tool")),
            "a refused tool says why: {mixed}"
        );
        assert_eq!(mixed["tool_availability"].as_array().map(Vec::len), Some(2));
    }
}
