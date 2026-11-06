//! Observations at the actual operation boundary, never execution authority.

use serde::{Deserialize, Serialize};

use super::OperationRefusalKind;
use crate::ops::{AsyncOpRef, ToolDispatchOutcome, ToolDispatchTerminalErrorKind};

/// Stage whose observation could not be retained after the real operation.
/// A diagnostic never changes the operation's returned result or permits retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OperationObservationPhase {
    Outcome,
}

/// What the adapter actually observed. A returned dispatch may have started
/// deferred work; it is not proof that every physical effect completed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationObservedOutcome {
    ToolDispatchReturned {
        asynchronous_operations: Vec<AsyncOpRef>,
        result_is_error: bool,
        terminal_error: Option<ToolDispatchTerminalErrorKind>,
    },
    /// The call returned an error. Its physical effect remains unknown unless
    /// a separate actual operation owner supplies a stronger observation.
    ToolDispatchError {
        error: ToolDispatchTerminalErrorKind,
    },
    /// The transport returned headers, not a complete model response.
    HttpResponse { status: u16 },
    /// A send/receive failure does not prove that the remote request did not run.
    TransportError,
    /// The Session owner returned an append disposition. Durability is still
    /// established by that owner's existing session commit, not this value.
    ContextAppendReturned {
        status: crate::service::AppendSystemContextStatus,
    },
    /// The Session control call failed. This is not a permission verdict or
    /// evidence that every attempted mutation was rolled back.
    ContextAppendError,
    /// A bounded original input was materialized inside its owner. This does
    /// not claim it was returned or disclosed to a model; final checks can
    /// still withhold the complete review context after this observation.
    SourceReadMaterialized,
    /// No complete source material was obtained. No source bytes are recorded.
    SourceReadUnavailable,
}

impl OperationObservedOutcome {
    #[must_use]
    pub fn from_tool_dispatch(result: &Result<ToolDispatchOutcome, crate::ToolError>) -> Self {
        match result {
            Ok(outcome) => Self::ToolDispatchReturned {
                asynchronous_operations: outcome.async_ops.clone(),
                result_is_error: outcome.result.is_error,
                terminal_error: outcome.terminal_cause().map(|cause| cause.kind()),
            },
            Err(error) => Self::ToolDispatchError {
                error: error.into(),
            },
        }
    }
}

impl std::fmt::Debug for OperationObservedOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OperationObservedOutcome([REDACTED])")
    }
}

/// The exact operation owner supplies these observations. They do not advance
/// any run, grant, tool, or input lifecycle and cannot be replayed as commands.
#[derive(Clone)]
pub enum OperationObservation {
    /// The actual review owner started this attempt on this exact candidate.
    /// This is historical association, never a verdict or entry allowance.
    /// One operation records two when the fenced dispatch reviews a decision
    /// a completed owner publication change superseded once more: the
    /// successor's `Prepared` record comes between them, and record order is
    /// the join (no link field is recorded).
    ReviewAttemptStarted {
        attempt_ref: crate::approval::review::ReviewAttemptRef,
    },
    /// A synchronous entry attempt at the named boundary. Several boundaries
    /// can observe the entry of one operation (each staging point on a tool
    /// call's dispatch chain, then its effect), so each names itself; none
    /// is proof that the physical effect ran. One boundary can also observe
    /// one operation twice: the fenced dispatch's root staging records its
    /// entry before the review check point finds the retained model review
    /// superseded, and the operation's second review attempt stages again.
    Entry(OperationEntryBoundary),
    Outcome(OperationObservedOutcome),
    Refused(OperationRefusalKind),
    AuthorizationUnavailable,
}

/// The operation boundary that observed an entry attempt. It identifies the
/// layer that staged the entry, so the entries of one operation can be told
/// apart; it grants nothing and is not a lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OperationEntryBoundary {
    /// A tool call's staging on its dispatch chain: the session's root
    /// dispatch, or an adapter on the way to the owning tool
    /// ([`crate::ToolDispatchContext::observe_tool_entry`]). The root
    /// staging records its entry between its first and final checks. When
    /// the final check refuses while an owner change is in progress, and the
    /// staging waits that change out and stages again, the operation records
    /// this boundary twice, the second time for the decision checked after
    /// the wait.
    ToolDispatch,
    /// The call-level execution policy gate's staging of a tool call
    /// ([`crate::ExecutionPolicyGatedDispatcher`]), after the gate's own
    /// awaits. Like the root staging, it records its entry between its first
    /// and final checks: when the final check refuses while an owner change
    /// is in progress, and the staging waits that change out and stages
    /// again, the operation records this boundary twice, the second time for
    /// the decision checked after the wait.
    ToolExecutionPolicy,
    /// The owning tool leaf's single consuming entry, immediately before
    /// its effect or transport handoff
    /// ([`crate::ToolDispatchContext::enter_reviewed_effect`]). A waiting
    /// entry
    /// ([`crate::ToolDispatchContext::enter_reviewed_effect_held_after_owner_change`])
    /// can record it after a refused staging of the same operation: a
    /// staging refused to rebuild while an owner change was in progress
    /// waits that change out (its wait traced at site `tool_entry`) and
    /// stages again, and the entry is recorded for the decision checked
    /// after the wait. A staging whose final check refused had already
    /// recorded its entry, so the operation then records this boundary
    /// twice, as at the root staging.
    ToolEffect,
    /// An MCP resource read's transport entry.
    McpResourceRead,
    /// Applying a live MCP attachment control.
    McpAttachment,
    /// A model request's transport entry.
    ModelRequest,
    /// A system context append to a session.
    ContextAppend,
    /// A source read for an operation's review context.
    ReviewContextRead,
}

impl std::fmt::Debug for OperationObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OperationObservation([REDACTED])")
    }
}

/// Known failure to stage an authoritative observation. Before entry this
/// prevents entry and is reported as infrastructure failure. After entry it
/// accompanies the original result; callers must not relabel a returned result
/// or retry its effect because of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("operation observation unavailable")]
pub struct OperationObservationError;

/// A protected observation owner's step before a governed tool call enters
/// its dispatcher.
///
/// A fenced dispatch whose context carries one
/// ([`crate::ToolDispatchContext::with_observation_barrier`]) awaits it after
/// the call's decision is prepared, any review admitted and the root entry
/// staged, and before any dispatcher code runs, so the owner can make what
/// its sink staged so far durable. The dispatch then checks the decision
/// again: a decision prepared again meanwhile (an owner change during the
/// wait) is staged with a new entry and the barrier is awaited once more, so
/// what the barrier made durable always carries the decision the call
/// enters with. An error refuses the call before any dispatcher code runs
/// ([`crate::ToolError::OperationObservationUnavailable`]); nothing is sent.
///
/// Only the native runtime installs one, for a governed service call: its
/// audit log appends the call's decision and entry here (see
/// `meerkat_runtime::meerkat_machine::ServiceCall`). Host code never
/// installs one. A barrier grants nothing, and it is no audit owner of its
/// own: the observations are the work's sink's. The fenced dispatch holds no
/// lock of its own while it waits, and the owner bounds its own wait.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait OperationObservationBarrier: Send + Sync {
    /// Make every observation staged so far durable, or refuse the entry.
    async fn before_entry(&self) -> Result<(), OperationObservationError>;
}

/// Bound on the already-run provider-native tools a withholding record lists.
pub const MANDATORY_HOOK_PROVIDER_NATIVE_TOOLS: usize = 16;
/// A provider-owned dynamic tool name longer than this is omitted, never
/// truncated into an exact-looking prefix.
pub const MANDATORY_HOOK_PROVIDER_NATIVE_NAME_BYTES: usize = 128;
/// A configured hook id longer than this is omitted.
pub const MANDATORY_HOOK_ID_BYTES: usize = 1024;

/// Typed historical facts that a mandatory hook withheld the dependent effects
/// of one model operation. Bounded: it carries no hook free text or payload,
/// no request and no response content. It is not a permission decision, and
/// decoding it grants nothing and replays nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MandatoryHookWithholding {
    /// The run-local model operation sequence minted by the agent loop.
    pub sequence: u64,
    /// Absent when the configured id exceeded the bound.
    pub hook_id: Option<crate::hooks::HookId>,
    pub hook_id_omitted: bool,
    pub point: crate::hooks::HookPoint,
    pub cause: MandatoryHookCause,
    pub effect: MandatoryHookWithheldEffect,
}

/// Why the mandatory prerequisite did not pass. A hook verdict or a typed
/// confinement refusal, never an authorization refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MandatoryHookCause {
    Denied {
        reason_code: crate::hooks::HookReasonCode,
    },
    LaunchRefused {
        refusal: crate::confinement::ConfinementRefusal,
    },
}

/// What was withheld, at the boundary where the hook actually ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MandatoryHookWithheldEffect {
    /// PreLlm: this model request was never sent.
    RequestNotSent,
    /// PostLlm: the response already exists. Only its later local tool
    /// dispatch and its conversation commit were withheld; transport,
    /// produced text and provider-native effects are not undone.
    LocalDispatchAndCommitWithheld {
        local_tool_calls: u64,
        /// A text body was produced; whether a delta reached a subscriber is
        /// not claimed here.
        text_output_produced: bool,
        provider_native_tools: Vec<MandatoryHookProviderTool>,
        provider_native_tools_total: u64,
        provider_native_tools_omitted: u64,
    },
}

/// One already-run provider-native tool, by typed kind. An oversized dynamic
/// name is replaced by an explicit omission marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MandatoryHookProviderTool {
    pub tool: Option<crate::types::ServerToolKind>,
    pub name_omitted: bool,
}

impl MandatoryHookProviderTool {
    /// Bounded projection of already-run provider-native tools, with the
    /// number of entries not listed.
    #[must_use]
    pub fn bounded(tools: &[crate::types::ServerToolKind]) -> (Vec<Self>, u64) {
        let listed = tools
            .iter()
            .take(MANDATORY_HOOK_PROVIDER_NATIVE_TOOLS)
            .map(|kind| match kind {
                crate::types::ServerToolKind::ProviderNative { name }
                    if name.len() > MANDATORY_HOOK_PROVIDER_NATIVE_NAME_BYTES =>
                {
                    Self {
                        tool: None,
                        name_omitted: true,
                    }
                }
                kind => Self {
                    tool: Some(kind.clone()),
                    name_omitted: false,
                },
            })
            .collect();
        let omitted = tools
            .len()
            .saturating_sub(MANDATORY_HOOK_PROVIDER_NATIVE_TOOLS);
        (listed, u64::try_from(omitted).unwrap_or(u64::MAX))
    }
}

/// A withholding observation bound to the actual model candidate and the
/// retained work's execution scope. Only the work context constructs it, so a
/// caller cannot choose another scope.
#[derive(Debug, Clone)]
pub struct MandatoryHookWithholdingRecord {
    operation_id: crate::OperationId,
    execution_scope: crate::exact_operation::OperationExecutionScope,
    run_id: Option<crate::RunId>,
    withholding: MandatoryHookWithholding,
}

impl MandatoryHookWithholdingRecord {
    pub(crate) fn new(
        operation_id: crate::OperationId,
        execution_scope: crate::exact_operation::OperationExecutionScope,
        run_id: Option<crate::RunId>,
        withholding: MandatoryHookWithholding,
    ) -> Self {
        Self {
            operation_id,
            execution_scope,
            run_id,
            withholding,
        }
    }

    /// The actual candidate operation, reserved before its pre-hook.
    pub fn operation_id(&self) -> &crate::OperationId {
        &self.operation_id
    }

    pub fn execution_scope(&self) -> &crate::exact_operation::OperationExecutionScope {
        &self.execution_scope
    }

    pub fn run_id(&self) -> Option<&crate::RunId> {
        self.run_id.as_ref()
    }

    pub fn withholding(&self) -> &MandatoryHookWithholding {
        &self.withholding
    }
}
