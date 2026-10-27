//! Operator verbs for oversized/wedged member transcripts.
//!
//! Two verbs, one incident class (a member transcript
//! grows past what a turn can carry and the only prior remedy was row
//! surgery):
//!
//! - `meta/compact_member` — the routine verb. Arms a temporary
//!   auto-compaction floor for the identity (bridge-applied via the typed
//!   `override_profile.auto_compact_threshold` carrier), rebuilds the member
//!   onto its SAME durable session (a retire, then a fresh embodiment, under
//!   one lifecycle lock) so a fresh build picks the floor up, drives one
//!   queued maintenance turn so the forced compaction fires at that turn's
//!   pre-LLM boundary, then disarms the floor and rebuilds again so the live
//!   build returns to the original profile value. Leaving the floor armed
//!   would compact every turn — the meerkat override is deliberately
//!   non-durable across recovery, so the profile is what re-supplies it on
//!   every build.
//!
//! - `meta/bound_member_transcript` — the surgical escape hatch. Commits
//!   one audited keep-last-N rewrite through meerkat's
//!   `SessionServiceTranscriptEditExt` on the CONCRETE persistent session
//!   service (the erased `dyn MobSessionService` cannot reach the extension
//!   trait — no trait upcasting), with a pair-safe cut point that never
//!   separates an assistant tool-use message from its adjacent tool_results.
//!   The service refuses live/running sessions with `SessionError::Busy`
//!   (`TranscriptEditRunningBehavior` has only `Reject`); that surfaces as
//!   this verb's typed refusal — quiesce the member first (retire/park), then
//!   retry. The cut knowingly drops EVERYTHING before it, the system prompt
//!   and any detached `fork_off`/council completion entries included; the
//!   response reports how many completion entries went
//!   (`dropped_completion_entries`) so that loss is never silent.
//!
//! Both verbs resolve their target through the same identity-control gate the
//! neighboring destructive verbs (`meta/respawn`, `meta/reset`,
//! `meta/retire`) use, so identities the gateway does not own are refused
//! at resolution.
//!
//! A third pair, the member's standing instructions (a restored member
//! otherwise keeps running the prompt minted with its session after its
//! skills changed, because resume authors nothing):
//!
//! - `meta/member_activate_instruction` appends one keyed, immutable
//!   instruction activation to the member's CURRENT session through Meerkat's
//!   member-level door, `MobHandle::activate_member_instruction`. It holds the
//!   session's runtime turn-finalization boundary and applies the shared
//!   runtime admission. History is never rewritten: the activation is a new
//!   transcript row, and re-applying the effective activation is a typed
//!   `duplicate`. A call made mid-turn BLOCKS until that turn finalizes (it
//!   can wait for a whole turn) and applies at the boundary, never inside the
//!   turn. If work is still queued at the boundary, or there is no session or
//!   an open live channel, the refusal goes back to the host as a typed
//!   admission class. No read deadline wraps this mutation, and nothing
//!   retries here.
//! - `meta/member_instruction_activations` reads the durable activation
//!   records of the member's current session.
//!
//! All of these are host verbs: they exist only on the gateway's host
//! JSON-RPC surface and its SDKs, never in an agent tool catalog.

use super::*;
use crate::identity_first::AgentIdentity;
use meerkat_core::SessionError;
use meerkat_core::types::{Message, SystemNoticeKind, SystemNoticeMessage};
use std::num::NonZeroU64;

/// Typed refusal: the target session is live/running and transcript surgery
/// only supports `Reject` while work is active. Distinct from the SDKs'
/// reserved `-32004` capability code and the identity-plane `-32001..-32005`
/// band.
pub const OPERATOR_SESSION_BUSY_CODE: i64 = -32015;

/// The verb exists on this gateway build but its wiring is absent (no
/// compaction-floor registry / no concrete transcript-edit service threaded
/// into the RPC context).
pub const OPERATOR_VERB_UNAVAILABLE_CODE: i64 = -32016;

/// Default temporary floor: small enough that any oversized transcript is
/// past it, large enough to be a legal non-zero threshold.
const DEFAULT_COMPACT_FLOOR_TOKENS: u64 = 1024;

/// Default budget for the forced-compaction maintenance turn.
const DEFAULT_COMPACT_TIMEOUT_MS: u64 = 60_000;

/// Secondary observation budget after the caller's deadline. Expiry reports
/// pending ownership; it does not cancel the exact-input observer or its cleanup.
const COMPACT_TIMEOUT_EVIDENCE_BUDGET: Duration = Duration::from_secs(5);

/// Default keep-last-N for `bound_member_transcript`.
const DEFAULT_BOUND_KEEP_LAST: usize = 50;

fn rpc_error(response_id: Value, code: i64, message: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message,
            data: None,
        }),
    }
}

fn rpc_result(response_id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: Some(result),
        error: None,
    }
}

fn optional_u64_param(params: &Value, field: &str) -> Result<Option<u64>, String> {
    match params.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_u64() {
            Some(parsed) if parsed > 0 => Ok(Some(parsed)),
            _ => Err(format!("{field} must be a positive integer")),
        },
    }
}

/// The largest prefix length that can be cut while keeping at least
/// `keep_last` trailing messages without orphaning a tool pair.
///
/// The transcript shape rule (meerkat-core `validate_transcript_tool_result_shape`)
/// makes tool pairs ADJACENT: every `ToolResults` is immediately preceded by
/// the assistant tool-use message carrying exactly its tool_use_id set, and
/// vice versa. A cut point is therefore pair-safe iff the first KEPT message
/// is not a `ToolResults`; walking the cut back one message re-includes the
/// paired assistant message. In a shape-valid transcript one step suffices
/// (two adjacent `ToolResults` are impossible); the loop is defensive.
pub(crate) fn pair_safe_cut_index(messages: &[Message], keep_last: usize) -> usize {
    let mut cut = messages.len().saturating_sub(keep_last);
    while cut > 0 && matches!(messages.get(cut), Some(Message::ToolResults { .. })) {
        cut -= 1;
    }
    cut
}

/// How many detached job completion entries lie before `cut`, the rows a
/// keep-last-N bound drops. Recognized by the typed completion marker only.
pub(crate) fn completion_entries_before_cut(messages: &[Message], cut: usize) -> usize {
    messages
        .iter()
        .take(cut)
        .filter(|message| crate::detached_completion::is_detached_completion_entry(message))
        .count()
}

struct ResolvedOperatorTarget {
    identity: AgentIdentity,
    /// Alias-pinned lifecycle precondition, mirroring `meta/respawn`:
    /// present only when the caller addressed the member by its reserved
    /// generated runtime alias.
    expected_alias: Option<String>,
}

/// Shared target resolution + authorization for both operator verbs: the
/// exact identity-control gate the destructive identity verbs use, including
/// the stale-live-alias refusal.
async fn resolve_operator_target(
    runtime: &UnifiedRuntime,
    identity_rt: &crate::identity_first::IdentityRuntime,
    params: &Value,
    response_id: &Value,
) -> Result<ResolvedOperatorTarget, Box<JsonRpcResponse>> {
    let identity_str = params
        .get("identity")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let target = match resolve_rpc_identity_control_target(runtime, identity_rt, identity_str).await
    {
        Ok(target) => target,
        Err(error) => return Err(Box::new(error.into_response(response_id.clone()))),
    };
    if let Some(response) =
        rpc_stale_live_alias_error_response(identity_rt, &target, response_id.clone()).await
    {
        return Err(Box::new(response));
    }
    let expected_alias = crate::member_comms_id::is_reserved_generated_alias(identity_str)
        .then(|| identity_str.to_string());
    Ok(ResolvedOperatorTarget {
        identity: target.identity,
        expected_alias,
    })
}

/// Transcript facts read through the concrete session service: message count
/// plus the head revision and the most recent rewrite commit reason.
async fn read_transcript_facts(
    service: &Arc<dyn crate::memory::hygienist::TranscriptEditSessionService>,
    session_id: &meerkat_core::types::SessionId,
) -> Result<(usize, Option<String>, Option<String>), SessionError> {
    let page = service
        .read_history(
            session_id,
            meerkat_core::service::SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await?;
    let (head_revision, last_reason) = match service
        .list_transcript_revisions(
            session_id,
            meerkat_core::service::SessionTranscriptRevisionListQuery {
                limit: None,
                offset: None,
            },
        )
        .await
    {
        Ok(list) => (
            Some(list.head_revision),
            list.entries.last().map(|entry| entry.reason.clone()),
        ),
        Err(SessionError::Unsupported(_)) => (None, None),
        Err(err) => return Err(err),
    };
    Ok((page.messages.len(), head_revision, last_reason))
}

/// JSON-RPC code for a member instruction activation refused at its safe
/// boundary (runtime work still queued, open live channel, no materialized
/// session, a fenced store asking for backoff). The host retries at its own
/// next boundary.
pub const MEMBER_INSTRUCTION_REFUSED_CODE: i64 = OPERATOR_SESSION_BUSY_CODE;

/// The audit actor for host-invoked instruction activations.
const INSTRUCTION_ACTIVATION_ACTOR: &str = "host";

/// Typed JSON-RPC error for a `MemberInstructionActivationError`. The four
/// classes stay distinct in `data.class`; an admission refusal also carries
/// Meerkat's stable `instruction_activation_code`, and a session refusal the
/// session error's stable code plus, when the owner refused the request
/// itself, its typed `InstructionActivationErrorCode`. No class is retried
/// here.
fn member_instruction_error_response(
    response_id: Value,
    error: meerkat_mob::MemberInstructionActivationError,
) -> JsonRpcResponse {
    use meerkat_core::InstructionActivationAdmissionErrorCode as Code;
    use meerkat_core::InstructionActivationErrorCode as RefusalCode;
    use meerkat_mob::MemberInstructionActivationError as Error;
    #[derive(serde::Deserialize)]
    struct ActivationRefusal {
        instruction_activation_code: RefusalCode,
    }
    let message = error.to_string();
    let (code, data) = match &error {
        Error::Admission { code, .. } => {
            let rpc_code = match code {
                Code::UnsupportedCurrentLowering => -32602,
                Code::DurabilityUnavailable => -32603,
                Code::TargetNotMaterialized
                | Code::LiveChannelOpen
                | Code::SessionBusy
                | Code::ExternalWriteFenceConflict
                | Code::ExternalWriteFenceBackoff => MEMBER_INSTRUCTION_REFUSED_CODE,
            };
            (
                rpc_code,
                serde_json::json!({
                    "class": "admission",
                    "instruction_activation_code": code,
                }),
            )
        }
        Error::Session(session_error) => {
            // The session owner reports a refused request as
            // `FailedWithData` carrying its typed `InstructionActivationErrorCode`;
            // that payload is deserialized, never read out of the message.
            let activation_code = match session_error {
                SessionError::FailedWithData { data, .. } => {
                    serde_json::from_value::<ActivationRefusal>(data.clone())
                        .ok()
                        .map(|refusal| refusal.instruction_activation_code)
                }
                _ => None,
            };
            let rpc_code = match (session_error, activation_code) {
                (SessionError::Busy { .. }, _) => MEMBER_INSTRUCTION_REFUSED_CODE,
                (_, Some(RefusalCode::MalformedActivationHistory) | None) => -32603,
                // The request itself is wrong for the member's current
                // activation state: the host fixes it, nothing retries.
                (_, Some(_)) => -32602,
            };
            let mut data = serde_json::json!({
                "class": "session",
                "session_error_code": session_error.code(),
            });
            if let Some(activation_code) = activation_code {
                data["instruction_activation_error_code"] = serde_json::json!(activation_code);
            }
            (rpc_code, data)
        }
        Error::Runtime(_) => (-32603, serde_json::json!({ "class": "runtime" })),
        Error::OwnerTask(_) => (-32603, serde_json::json!({ "class": "owner_task" })),
        // `#[non_exhaustive]`: a class this build does not know is still a
        // typed failure, never a success.
        _ => (-32603, serde_json::json!({ "class": "unknown" })),
    };
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message,
            data: Some(data),
        }),
    }
}

/// `meta/member_activate_instruction`: append one keyed instruction
/// activation to the member's current session at a safe boundary.
///
/// Params: `identity` (the member, resolved through the operator gate) and
/// `activation`, Meerkat's `InstructionActivationRequest` verbatim. The result
/// is `{identity, receipt}` with Meerkat's `InstructionActivationReceipt`
/// (`disposition` is `applied` or `duplicate`).
pub(super) async fn handle_member_activate_instruction(
    runtime: &UnifiedRuntime,
    ctx: &IdentityFirstContext,
    params: &Value,
    response_id: Value,
) -> JsonRpcResponse {
    let activation: meerkat_core::InstructionActivationRequest =
        match params.get("activation").cloned() {
            Some(value) => match serde_json::from_value(value) {
                Ok(activation) => activation,
                Err(error) => {
                    return rpc_error(response_id, -32602, format!("invalid activation: {error}"));
                }
            },
            None => {
                return rpc_error(response_id, -32602, "activation is required".to_string());
            }
        };
    let target =
        match resolve_operator_target(runtime, ctx.runtime.runtime(), params, &response_id).await {
            Ok(target) => target,
            Err(response) => return *response,
        };
    let member = crate::member_comms_id::mob_member_id(target.identity.as_str());
    let namespace = activation.revision.namespace.clone();
    let key = activation.revision.key.clone();
    let activation_id = activation.activation_id.clone();
    match runtime
        .mob_handle()
        .activate_member_instruction(&member, activation)
        .await
    {
        Ok(receipt) => {
            tracing::info!(
                actor = INSTRUCTION_ACTIVATION_ACTOR,
                identity = %target.identity,
                namespace = %namespace,
                key = %key,
                activation_id = %activation_id,
                disposition = ?receipt.disposition,
                session_id = %receipt.record.session_id,
                transcript_revision = %receipt.record.projection_witness.transcript_revision,
                "host activated a member instruction"
            );
            match serde_json::to_value(&receipt) {
                Ok(receipt) => rpc_result(
                    response_id,
                    serde_json::json!({
                        "identity": target.identity.as_str(),
                        "receipt": receipt,
                    }),
                ),
                Err(error) => rpc_error(
                    response_id,
                    -32603,
                    format!("failed to serialize the activation receipt: {error}"),
                ),
            }
        }
        Err(error) => {
            tracing::warn!(
                actor = INSTRUCTION_ACTIVATION_ACTOR,
                identity = %target.identity,
                namespace = %namespace,
                key = %key,
                activation_id = %activation_id,
                admission_code = ?error.admission_code(),
                error = %error,
                "host member instruction activation refused"
            );
            member_instruction_error_response(response_id, error)
        }
    }
}

/// `meta/member_instruction_activations`: read the durable instruction
/// activation records of the member's current session.
///
/// Params: `identity`, plus Meerkat's read query fields `namespace`, `key`,
/// `offset` and `limit` (default 100). The result is `{identity, page}` with
/// Meerkat's `InstructionActivationReadPage`.
pub(super) async fn handle_member_instruction_activations(
    runtime: &UnifiedRuntime,
    ctx: &IdentityFirstContext,
    params: &Value,
    response_id: Value,
) -> JsonRpcResponse {
    let mut query_fields = serde_json::Map::new();
    for field in ["namespace", "key", "offset", "limit"] {
        if let Some(value) = params.get(field).filter(|value| !value.is_null()) {
            query_fields.insert(field.to_string(), value.clone());
        }
    }
    let query: meerkat_core::InstructionActivationReadQuery =
        match serde_json::from_value(Value::Object(query_fields)) {
            Ok(query) => query,
            Err(error) => {
                return rpc_error(response_id, -32602, format!("invalid query: {error}"));
            }
        };
    let target =
        match resolve_operator_target(runtime, ctx.runtime.runtime(), params, &response_id).await {
            Ok(target) => target,
            Err(response) => return *response,
        };
    let member = crate::member_comms_id::mob_member_id(target.identity.as_str());
    match runtime
        .mob_handle()
        .read_member_instruction_activations(&member, query)
        .await
    {
        Ok(page) => match serde_json::to_value(&page) {
            Ok(page) => rpc_result(
                response_id,
                serde_json::json!({ "identity": target.identity.as_str(), "page": page }),
            ),
            Err(error) => rpc_error(
                response_id,
                -32603,
                format!("failed to serialize the activation page: {error}"),
            ),
        },
        Err(error) => member_instruction_error_response(response_id, error),
    }
}

/// `meta/compact_member`: force one compaction on a member's next turn via
/// a temporary profile-level threshold floor, then restore the original
/// threshold.
pub(super) async fn handle_compact_member(
    runtime: &UnifiedRuntime,
    ctx: &IdentityFirstContext,
    params: &Value,
    response_id: Value,
) -> JsonRpcResponse {
    let identity_rt = ctx.runtime.runtime();
    let Some(floors) = ctx.compaction_floors.as_ref() else {
        return rpc_error(
            response_id,
            OPERATOR_VERB_UNAVAILABLE_CODE,
            "compact_member is not wired on this gateway: the identity bridge's compaction-floor \
             registry was not threaded into the RPC context"
                .to_string(),
        );
    };
    let floor_tokens = match optional_u64_param(params, "floor_tokens") {
        Ok(value) => value.unwrap_or(DEFAULT_COMPACT_FLOOR_TOKENS),
        Err(message) => return rpc_error(response_id, -32602, message),
    };
    let Some(floor) = NonZeroU64::new(floor_tokens) else {
        return rpc_error(
            response_id,
            -32602,
            "floor_tokens must be greater than 0".to_string(),
        );
    };
    let timeout_ms = match optional_u64_param(params, "timeout_ms") {
        Ok(value) => value.unwrap_or(DEFAULT_COMPACT_TIMEOUT_MS),
        Err(message) => return rpc_error(response_id, -32602, message),
    };
    let target = match resolve_operator_target(runtime, identity_rt, params, &response_id).await {
        Ok(target) => target,
        Err(response) => return *response,
    };
    let Some(observer) = runtime
        .mob_runtime()
        .committed_boundary_recoverer()
        .and_then(|owner| owner.runtime_completion_observer())
    else {
        return rpc_error(
            response_id,
            OPERATOR_VERB_UNAVAILABLE_CODE,
            "compact_member requires the persistent session owner's exact completion observer"
                .to_string(),
        );
    };
    let operation = CompactOperation {
        ctx: ctx.clone(),
        identity: target.identity,
        expected_alias: target.expected_alias,
        observer,
        admission: Arc::new(IdentityCompactionAdmission),
        floors: Arc::clone(floors),
        floor,
        timeout_ms,
        response_id: response_id.clone(),
        operation_id: meerkat_core::SessionId::new().to_string(),
    };
    let (early_tx, mut early_rx) = tokio::sync::oneshot::channel();
    let owned =
        identity_rt.run_tracked_foreground(async move { Ok(operation.run(early_tx).await) });
    tokio::pin!(owned);
    tokio::select! {
        biased;
        Ok(response) = &mut early_rx => response,
        result = &mut owned => match result {
            Ok(response) => response,
            Err(error) => identity_error_response(response_id, &error),
        }
    }
}

#[async_trait::async_trait]
trait CompactCompletionObserver: Send + Sync {
    async fn observe(
        &self,
        session_id: &meerkat_core::SessionId,
        key: &str,
        input_id: &mut Option<meerkat_core::lifecycle::InputId>,
    ) -> Result<Option<meerkat_runtime::CompletionOutcome>, meerkat_runtime::RuntimeDriverError>;
}

#[async_trait::async_trait]
trait CompactAdmission: Send + Sync {
    async fn dispatch(
        &self,
        runtime: &crate::identity_first::IdentityRuntime,
        identity: &AgentIdentity,
        incarnation: &crate::identity_first::runtime::CapturedIncarnation,
        input: &crate::identity_first::DispatchInput,
    ) -> Result<
        crate::identity_first::runtime::DispatchOutcome,
        crate::identity_first::IdentityRuntimeError,
    >;
}

struct IdentityCompactionAdmission;

#[async_trait::async_trait]
impl CompactAdmission for IdentityCompactionAdmission {
    async fn dispatch(
        &self,
        runtime: &crate::identity_first::IdentityRuntime,
        identity: &AgentIdentity,
        incarnation: &crate::identity_first::runtime::CapturedIncarnation,
        input: &crate::identity_first::DispatchInput,
    ) -> Result<
        crate::identity_first::runtime::DispatchOutcome,
        crate::identity_first::IdentityRuntimeError,
    > {
        runtime
            .dispatch_with_expected_incarnation(identity, None, Some(incarnation), input)
            .await
    }
}

fn compaction_admission_is_uncertain(error: &crate::identity_first::IdentityRuntimeError) -> bool {
    use crate::identity_first::{ActorCallObservation, IdentityRuntimeError};
    !matches!(
        error,
        IdentityRuntimeError::AdmissionBacklogFull { .. }
            | IdentityRuntimeError::ReloadRequired { .. }
            | IdentityRuntimeError::IdentityUnresumable { .. }
            | IdentityRuntimeError::IdentityHistoryUnavailable { .. }
            | IdentityRuntimeError::IdentityWakeRetryable { .. }
            | IdentityRuntimeError::IdentityHostAdopted { .. }
            | IdentityRuntimeError::WakeHeld { .. }
            | IdentityRuntimeError::InvalidState { .. }
            | IdentityRuntimeError::StaleRuntimeAlias { .. }
            | IdentityRuntimeError::PostAdmissionSuperseded { .. }
            | IdentityRuntimeError::ActorLoopStalled {
                observation: ActorCallObservation::BeforeCall,
                ..
            }
            | IdentityRuntimeError::ActorTerminated {
                observation: ActorCallObservation::BeforeCall,
                ..
            }
    )
}

#[async_trait::async_trait]
impl CompactCompletionObserver for meerkat_runtime::MeerkatMachine {
    async fn observe(
        &self,
        session_id: &meerkat_core::SessionId,
        key: &str,
        input_id: &mut Option<meerkat_core::lifecycle::InputId>,
    ) -> Result<Option<meerkat_runtime::CompletionOutcome>, meerkat_runtime::RuntimeDriverError>
    {
        use meerkat_runtime::SessionServiceRuntimeExt;
        if input_id.is_none() {
            *input_id = self
                .input_state_by_idempotency_key(session_id, key)
                .await?
                .map(|state| state.state.input_id);
        }
        match input_id.as_ref() {
            Some(id) => self.input_terminal_completion(session_id, id).await,
            None => Ok(None),
        }
    }
}

struct CompactOperation {
    ctx: IdentityFirstContext,
    identity: AgentIdentity,
    expected_alias: Option<String>,
    observer: Arc<dyn CompactCompletionObserver>,
    admission: Arc<dyn CompactAdmission>,
    floors: Arc<crate::identity_first::CompactionFloorRegistry>,
    floor: NonZeroU64,
    timeout_ms: u64,
    response_id: Value,
    operation_id: String,
}

impl CompactOperation {
    fn error(&self, kind: &str, stage: &str, detail: impl std::fmt::Display) -> JsonRpcResponse {
        self.error_with_data(kind, stage, detail, serde_json::Map::new())
    }

    fn error_with_data(
        &self,
        kind: &str,
        stage: &str,
        detail: impl std::fmt::Display,
        mut data: serde_json::Map<String, Value>,
    ) -> JsonRpcResponse {
        tracing::warn!(identity = %self.identity, operation_id = %self.operation_id,
            kind, stage, %detail, "compaction operation did not report completed restoration");
        data.insert("kind".to_string(), serde_json::json!(kind));
        data.insert("stage".to_string(), serde_json::json!(stage));
        data.insert(
            "operation_id".to_string(),
            serde_json::json!(self.operation_id),
        );
        data.insert(
            "identity".to_string(),
            serde_json::json!(self.identity.as_str()),
        );
        JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: self.response_id.clone(),
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: detail.to_string(),
                data: Some(Value::Object(data)),
            }),
        }
    }

    async fn run(self, early_tx: tokio::sync::oneshot::Sender<JsonRpcResponse>) -> JsonRpcResponse {
        let Ok(_operation_guard) = self.floors.operation_lock(&self.identity).try_lock_owned()
        else {
            return self.error(
                "compact_member_busy",
                "admission",
                "compaction already owns this member",
            );
        };
        if self.floors.get(&self.identity).is_some() {
            return self.error(
                "compact_member_busy",
                "admission",
                "a prior compaction floor remains unsettled",
            );
        }
        let runtime = self.ctx.runtime.runtime();
        let initial = match runtime.capture_incarnation(&self.identity).await {
            Ok(initial) => initial,
            Err(error) => return identity_error_response(self.response_id.clone(), &error),
        };
        let before_session = match runtime.status(&self.identity).await {
            Ok(status) => match status.session_id {
                Some(session) => session,
                None => {
                    return self.error(
                        "compact_member_refused",
                        "before",
                        "member has no durable session",
                    );
                }
            },
            Err(error) => return identity_error_response(self.response_id.clone(), &error),
        };
        let before = match self.ctx.transcript_edit_service.as_ref() {
            Some(service) => match tokio::time::timeout(
                COMPACT_TIMEOUT_EVIDENCE_BUDGET,
                read_transcript_facts(service, &before_session),
            )
            .await
            {
                Ok(Ok(facts)) => Some(facts),
                Ok(Err(error)) => {
                    return self.error("compact_member_observation_failed", "before", error);
                }
                Err(_) => {
                    return self.error(
                        "compact_member_observation_failed",
                        "before",
                        "preparation transcript read timed out; no floor installed",
                    );
                }
            },
            None => None,
        };
        let (session_id, incarnation) = match runtime
            .rebuild_compaction_owner(
                &self.identity,
                &initial,
                self.expected_alias.as_deref(),
                crate::identity_first::bridge::CompactionFloorChange {
                    registry: &self.floors,
                    operation_id: &self.operation_id,
                    floor: Some(self.floor),
                },
            )
            .await
        {
            Ok(owner) => owner,
            Err(error) => {
                return self.error("compact_member_rebuild_failed", "floor_install", error);
            }
        };
        let key = format!("meta-compact:{}", self.operation_id);
        let input = crate::identity_first::DispatchInput::system(
            "[meta-gateway operator verb compact_member] Maintenance turn: transcript compaction \
             was forced for this turn. Reply with a brief acknowledgement only.",
        )
        .with_idempotency(&key)
        .with_correlation(&self.operation_id);
        let mut early_tx = Some(early_tx);
        let mut had_observation_failure = false;
        let observed_incarnation = match self
            .admission
            .dispatch(runtime, &self.identity, &incarnation, &input)
            .await
        {
            Ok(admission) if admission.session_id.as_ref() == Some(&session_id) => {
                admission.incarnation
            }
            Ok(_) => {
                runtime
                    .invalidate_superseded_compaction_floor(
                        &self.identity,
                        &incarnation,
                        &self.floors,
                        &self.operation_id,
                    )
                    .await;
                return self.error(
                    "compact_member_superseded",
                    "dispatch",
                    "maintenance session changed",
                );
            }
            Err(error) if compaction_admission_is_uncertain(&error) => {
                had_observation_failure = true;
                let mut data = serde_json::Map::new();
                if let Some(observation) = error.structured_data() {
                    data.insert("admission_observation".to_string(), observation);
                }
                if let Some(sender) = early_tx.take() {
                    let response = self.error_with_data(
                        "compact_member_admission_pending", "admission_pending",
                        format!("maintenance admission outcome is unknown: {error}; original key and cleanup ownership retained without resubmission"),
                        data,
                    );
                    let _ = sender.send(response);
                }
                incarnation
            }
            Err(error) => {
                self.floors.clear_owned(&self.identity, &self.operation_id);
                return self.error("compact_member_admission_failed", "dispatch", error);
            }
        };
        let deadline = tokio::time::Instant::now() + Duration::from_millis(self.timeout_ms);
        let secondary_deadline = deadline + COMPACT_TIMEOUT_EVIDENCE_BUDGET;
        let mut timed_out = false;
        let mut input_id = None;
        let outcome = loop {
            if !matches!(
                runtime.capture_incarnation(&self.identity).await,
                Ok(current) if current == observed_incarnation
            ) {
                runtime
                    .invalidate_superseded_compaction_floor(
                        &self.identity,
                        &observed_incarnation,
                        &self.floors,
                        &self.operation_id,
                    )
                    .await;
                return self.error(
                    "compact_member_superseded",
                    "terminal",
                    "maintenance incarnation changed; no rebuild attempted",
                );
            }
            let now = tokio::time::Instant::now();
            timed_out |= now >= deadline;
            if now >= secondary_deadline
                && let Some(sender) = early_tx.take()
            {
                let response = self.error(
                    "compact_member_timeout", "terminal_pending",
                    format!("compact_member maintenance turn did not complete within {}ms; exact terminal is pending/unknown; runtime retains completion and profile-cleanup ownership, no retire or rebuild attempted", self.timeout_ms),
                );
                let _ = sender.send(response);
            }
            let observation = self.observer.observe(&session_id, &key, &mut input_id);
            match tokio::time::timeout(COMPACT_TIMEOUT_EVIDENCE_BUDGET, observation).await {
                Ok(Ok(Some(outcome))) => {
                    timed_out |= tokio::time::Instant::now() >= deadline;
                    break outcome;
                }
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    had_observation_failure = true;
                    tracing::warn!(identity = %self.identity, operation_id = %self.operation_id,
                        %error, "exact compaction terminal read failed; ownership retained");
                    if let Some(sender) = early_tx.take() {
                        let response = self.error(
                            "compact_member_observation_failed", "terminal_pending",
                            format!("exact compaction terminal is unknown after read failure: {error}; runtime retains completion and profile-cleanup ownership, no retire or rebuild attempted"),
                        );
                        let _ = sender.send(response);
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(_) => {
                    tracing::warn!(identity = %self.identity, operation_id = %self.operation_id,
                        "exact compaction terminal read timed out; ownership retained");
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let failure_metadata = match &outcome {
            meerkat_runtime::CompletionOutcome::Abandoned { error, .. }
            | meerkat_runtime::CompletionOutcome::AbandonedWithError { error, .. }
            | meerkat_runtime::CompletionOutcome::CompletedWithFinalizationFailure { error }
            | meerkat_runtime::CompletionOutcome::RuntimeTerminated { error, .. } => {
                Some(serde_json::json!(error))
            }
            _ => None,
        };
        let terminal_error = compaction_terminal_class(&outcome, &session_id);
        if let Some(class) = terminal_error {
            let deadline_detail = if timed_out {
                format!(
                    "maintenance turn did not complete within {}ms; ",
                    self.timeout_ms
                )
            } else {
                String::new()
            };
            let mut data = serde_json::Map::new();
            data.insert("completion_type".to_string(), serde_json::json!(class));
            data.insert(
                "observation_timed_out".to_string(),
                serde_json::json!(timed_out),
            );
            if let Some(error) = failure_metadata {
                data.insert("error".to_string(), error);
            }
            return self.error_with_data("compact_member_completion_failed", "terminal", format!(
                "compact_member {deadline_detail}exact input ended as {class}; profile restoration was not authorized"
            ), data);
        }
        if let Err(error) = runtime
            .rebuild_compaction_owner(
                &self.identity,
                &observed_incarnation,
                None,
                crate::identity_first::bridge::CompactionFloorChange {
                    registry: &self.floors,
                    operation_id: &self.operation_id,
                    floor: None,
                },
            )
            .await
        {
            runtime
                .invalidate_superseded_compaction_floor(
                    &self.identity,
                    &observed_incarnation,
                    &self.floors,
                    &self.operation_id,
                )
                .await;
            return self.error("compact_member_rebuild_failed", "profile_restore", error);
        }
        if timed_out {
            return self.error(
                "compact_member_timeout", "profile_restored",
                format!("compact_member maintenance turn did not complete within {}ms; the exact maintenance input completed after the observation deadline; member rebuilt at its original threshold", self.timeout_ms),
            );
        }
        if had_observation_failure {
            return self.error(
                "compact_member_observation_failed", "profile_restored",
                "maintenance observation failed before exact completion; the original input later completed and its profile was restored",
            );
        }
        let after = match self.ctx.transcript_edit_service.as_ref() {
            Some(service) => match read_transcript_facts(service, &session_id).await {
                Ok(facts) => Some(facts),
                Err(error) => {
                    return self.error("compact_member_observation_failed", "after", error);
                }
            },
            None => None,
        };
        let messages_before = before.as_ref().map(|(count, _, _)| *count);
        let messages_after = after.as_ref().map(|(count, _, _)| *count);
        rpc_result(
            self.response_id,
            serde_json::json!({
                "identity": self.identity.as_str(),
                "session_id": session_id.to_string(),
                "floor_tokens": self.floor.get(),
                "messages_before": messages_before,
                "messages_after": messages_after,
                "compaction_applied": messages_before.zip(messages_after).map(|(before, after)| after < before),
                "head_revision": after.as_ref().and_then(|(_, head, _)| head.clone()),
                "last_rewrite_reason": after.as_ref().and_then(|(_, _, reason)| reason.clone()),
            }),
        )
    }
}

/// Tear the identity's live member down and re-materialize it onto the SAME
/// durable session, forcing a fresh agent build.
///
/// A test path for the quiesce-and-rebuild cycle: retire tears the member
/// down to roster absence with the durable session preserved; `restore_flow`
/// then re-registers the identity from its current spec and resumes the SAME
/// session through the bridge's spawn-spec build, where a build-time input
/// like the armed compaction floor applies. Identity respawn
/// (`respawn_identity_in_place_tracked`) rebuilds too: it retires the
/// identity's Core member and wakes it onto the same session at a new Core
/// generation (see `IdentityRuntime::respawn_locked`).
///
/// A Dormant/never-materialized identity skips the retire and goes straight
/// to materialization. Deliberately NOT `restore_flow`: that is fleet-scoped
/// (it overwrites the runtime's desired peer edges from the roster it is
/// handed), while this verb must touch exactly one identity. The wake resumes
/// Core's last binding of the identity; a session Core refuses to
/// resume fails typed rather than starting fresh.
#[cfg(test)]
async fn rebuild_member_for_fresh_build(
    ctx: &IdentityFirstContext,
    identity: &AgentIdentity,
    expected_alias: Option<&str>,
    spec: crate::identity_first::DurableAgentSpec,
) -> Result<meerkat_core::types::SessionId, String> {
    use crate::identity_first::IdentityLifecycleState;
    let identity_rt = ctx.runtime.runtime();
    let state = identity_rt
        .status(identity)
        .await
        .map_err(|err| format!("status before rebuild: {err}"))?
        .state;
    if state == IdentityLifecycleState::Active {
        match expected_alias {
            Some(alias) => {
                identity_rt
                    .retire_member_alias_tracked(identity, alias)
                    .await
            }
            None => identity_rt.retire_tracked(identity).await,
        }
        .map_err(|err| format!("quiesce retire: {err}"))?;
    }
    // Adopt the current spec; the wake resumes Core's last binding of the
    // identity, the SAME durable session, through a fresh build.
    identity_rt.register(spec).await;
    identity_rt
        .materialize_tracked(identity)
        .await
        .map_err(|err| format!("re-materialization: {err}"))?
        .map(|scope| scope.session_id().clone())
        .ok_or_else(|| {
            format!(
                "identity {} has no Core session to rebuild onto",
                identity.as_str()
            )
        })
}

/// `meta/bound_member_transcript`: one audited keep-last-N transcript
/// rewrite on a quiesced member session.
///
/// The conversation continues from the N most recent messages; everything
/// before the cut is dropped, the system prompt and detached job completion
/// entries included. `dropped_completion_entries` in the response counts the
/// completion entries that went with it.
pub(super) async fn handle_bound_member_transcript(
    runtime: &UnifiedRuntime,
    ctx: &IdentityFirstContext,
    params: &Value,
    response_id: Value,
) -> JsonRpcResponse {
    let identity_rt = ctx.runtime.runtime();
    let Some(service) = ctx.transcript_edit_service.as_ref() else {
        return rpc_error(
            response_id,
            OPERATOR_VERB_UNAVAILABLE_CODE,
            "bound_member_transcript is not wired on this gateway: the concrete persistent \
             session service was not threaded into the RPC context (the erased MobSessionService \
             cannot reach SessionServiceTranscriptEditExt)"
                .to_string(),
        );
    };
    let keep_last = match optional_u64_param(params, "keep_last") {
        Ok(value) => value.map_or(DEFAULT_BOUND_KEEP_LAST, |parsed| parsed as usize),
        Err(message) => return rpc_error(response_id, -32602, message),
    };
    let note = match params.get("note") {
        None | Some(Value::Null) => None,
        Some(Value::String(note)) => Some(note.clone()),
        Some(_) => {
            return rpc_error(
                response_id,
                -32602,
                "note must be a string when provided".to_string(),
            );
        }
    };
    let target = match resolve_operator_target(runtime, identity_rt, params, &response_id).await {
        Ok(target) => target,
        Err(response) => return *response,
    };
    let identity = target.identity;

    let status = match identity_rt.status(&identity).await {
        Ok(status) => status,
        Err(err) => return identity_error_response(response_id, &err),
    };
    let Some(session_id) = status.session_id else {
        return rpc_error(
            response_id,
            -32000,
            format!(
                "identity {} has no current session to bound",
                identity.as_str()
            ),
        );
    };

    let messages = match service
        .read_history(
            &session_id,
            meerkat_core::service::SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
    {
        Ok(page) => page.messages,
        Err(err) => {
            return rpc_error(
                response_id,
                -32000,
                format!("bound_member_transcript failed to read the transcript: {err}"),
            );
        }
    };
    // Fresh head for the compare-and-swap: the rewrite is rejected if the
    // head advances between this read and the commit.
    let expected_parent_revision = match service
        .list_transcript_revisions(
            &session_id,
            meerkat_core::service::SessionTranscriptRevisionListQuery {
                limit: Some(0),
                offset: None,
            },
        )
        .await
    {
        Ok(list) => Some(list.head_revision),
        Err(SessionError::Unsupported(_)) => None,
        Err(err) => {
            return rpc_error(
                response_id,
                -32000,
                format!("bound_member_transcript failed to read the head revision: {err}"),
            );
        }
    };

    let cut = pair_safe_cut_index(&messages, keep_last);
    if cut == 0 {
        return rpc_result(
            response_id,
            serde_json::json!({
                "identity": identity.as_str(),
                "session_id": session_id.to_string(),
                "bounded": false,
                "removed": 0,
                "dropped_completion_entries": 0,
                "message_count": messages.len(),
            }),
        );
    }

    let dropped_completion_entries = completion_entries_before_cut(&messages, cut);
    let marker = Message::SystemNotice(SystemNoticeMessage::new(
        SystemNoticeKind::Generic,
        format!(
            "[operator] transcript bounded: {cut} earlier message(s) were removed by \
             bound_member_transcript; the conversation continues from the {} most recent \
             message(s).",
            messages.len() - cut
        ),
    ));
    let mut reason = meerkat_core::TranscriptRewriteReason::new("operator_bound_transcript");
    reason.note = Some(note.unwrap_or_else(|| {
        format!("meta-gateway operator verb bound_member_transcript keep_last={keep_last}")
    }));
    let request = meerkat_core::service::SessionTranscriptRewriteRequest {
        selection: meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: cut },
        replacement: vec![marker],
        reason,
        actor: Some("meta-gateway operator verb".to_string()),
        expected_parent_revision,
        running_behavior: meerkat_core::TranscriptEditRunningBehavior::default(),
    };
    match service
        .rewrite_session_transcript(&session_id, request)
        .await
    {
        Ok(result) => rpc_result(
            response_id,
            serde_json::json!({
                "identity": identity.as_str(),
                "session_id": result.session_id.to_string(),
                "bounded": true,
                "removed": cut,
                "dropped_completion_entries": dropped_completion_entries,
                "kept": messages.len() - cut,
                "message_count": result.message_count,
                "parent_revision": result.parent_revision,
                "revision": result.revision,
            }),
        ),
        Err(SessionError::Busy { id }) => rpc_error(
            response_id,
            OPERATOR_SESSION_BUSY_CODE,
            format!(
                "session {id} is live/running; transcript surgery only supports Reject while \
                 work is active - quiesce the member first (retire or park it), then retry"
            ),
        ),
        Err(err) => rpc_error(
            response_id,
            -32000,
            format!("bound_member_transcript rewrite failed: {err}"),
        ),
    }
}

/// The terminal class of a maintenance turn's exact input that refuses the
/// profile restoration, or None for its completion on its own session. Only
/// that completion proves the turn ran: every other outcome refuses, one a
/// recovery could not confirm (`outcome_unknown`) included, and no class
/// retries or redelivers the input.
fn compaction_terminal_class(
    outcome: &meerkat_runtime::CompletionOutcome,
    session_id: &meerkat_core::SessionId,
) -> Option<&'static str> {
    match outcome {
        meerkat_runtime::CompletionOutcome::Completed(result)
            if result.session_id == *session_id =>
        {
            None
        }
        meerkat_runtime::CompletionOutcome::Completed(_) => Some("wrong_session"),
        meerkat_runtime::CompletionOutcome::CompletedWithoutResult => {
            Some("completed_without_result")
        }
        meerkat_runtime::CompletionOutcome::CallbackPending { .. } => Some("callback_pending"),
        meerkat_runtime::CompletionOutcome::CallbackBatchPending { .. } => {
            Some("callback_batch_pending")
        }
        meerkat_runtime::CompletionOutcome::Cancelled => Some("cancelled"),
        meerkat_runtime::CompletionOutcome::Abandoned { .. } => Some("abandoned"),
        meerkat_runtime::CompletionOutcome::AbandonedWithError { .. } => {
            Some("abandoned_with_error")
        }
        meerkat_runtime::CompletionOutcome::CompletedWithFinalizationFailure { .. } => {
            Some("completed_with_finalization_failure")
        }
        meerkat_runtime::CompletionOutcome::RuntimeTerminated { .. } => Some("runtime_terminated"),
        meerkat_runtime::CompletionOutcome::OutcomeUnknown { .. } => Some("outcome_unknown"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    /// Only the exact input's own completion authorizes the rebuild. An
    /// outcome a recovery could not confirm refuses it under its own class,
    /// never as a success and never as a class that retries the input.
    #[test]
    fn an_unconfirmed_maintenance_outcome_refuses_the_rebuild() {
        let session_id = meerkat_core::SessionId::new();
        assert_eq!(
            super::compaction_terminal_class(
                &meerkat_runtime::CompletionOutcome::OutcomeUnknown {
                    run_id: meerkat_core::lifecycle::RunId::new(),
                },
                &session_id,
            ),
            Some("outcome_unknown")
        );
        assert_eq!(
            super::compaction_terminal_class(
                &meerkat_runtime::CompletionOutcome::Cancelled,
                &session_id
            ),
            Some("cancelled")
        );
    }

    use super::*;
    use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, ToolResult, UserMessage};

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    fn tool_use_pair(id: &str) -> (Message, Message) {
        let assistant = Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: id.to_string(),
                name: "probe".to_string(),
                args: serde_json::value::RawValue::from_string("{}".to_string()).expect("raw args"),
                meta: None,
            }],
            meerkat_core::types::StopReason::ToolUse,
        ));
        let results = Message::tool_results(vec![ToolResult {
            host_metadata: Default::default(),
            settlement_failures: Default::default(),
            tool_use_id: id.to_string(),
            content: vec![],
            is_error: false,
        }]);
        (assistant, results)
    }

    #[test]
    fn cut_keeps_whole_transcript_when_keep_last_covers_it() {
        let messages = vec![user("a"), user("b")];
        assert_eq!(pair_safe_cut_index(&messages, 2), 0);
        assert_eq!(pair_safe_cut_index(&messages, 10), 0);
    }

    /// A detached job's durable completion entry, exactly as meerkat's
    /// detached delivery builds it.
    fn completion_entry(job_id: &str) -> Message {
        Message::SystemNotice(
            meerkat_mob_mcp::detached_delivery::detached_completion_notice(
                "fork_off",
                job_id,
                meerkat_core::event::BackgroundJobTerminalStatus::Completed,
                &serde_json::json!({ "text": "done" }),
            )
            .expect("meerkat builds the completion notice"),
        )
    }

    fn mixed_completion_entry(job_id: &str) -> Message {
        let Message::SystemNotice(mut notice) = completion_entry(job_id) else {
            panic!("completion fixture must be a typed notice");
        };
        notice
            .blocks
            .push(meerkat_core::types::SystemNoticeBlock::BackgroundJob {
                job_id: "shell-refresh".to_string(),
                display_name: Some("shell".to_string()),
                status: meerkat_core::event::BackgroundJobTerminalStatus::Completed,
                detail: None,
                persisted: false,
            });
        Message::SystemNotice(notice)
    }

    #[test]
    fn dropped_completion_entries_count_a_mixed_notice_once() {
        let Message::SystemNotice(mixed) = mixed_completion_entry("job-a") else {
            panic!("mixed fixture must be a typed notice");
        };
        let refresh = Message::SystemNotice(SystemNoticeMessage::with_blocks(
            SystemNoticeKind::BackgroundJob,
            None,
            vec![mixed.blocks[1].clone()],
        ));
        let mut multiple_completions = mixed.clone();
        let Message::SystemNotice(second) = completion_entry("job-b") else {
            panic!("completion fixture must be a typed notice");
        };
        multiple_completions.blocks.extend(second.blocks);
        let messages = vec![
            Message::SystemNotice(mixed),
            refresh,
            Message::SystemNotice(SystemNoticeMessage::new(
                SystemNoticeKind::Generic,
                "Background fork_off job job-x finished (completed)",
            )),
            Message::SystemNotice(multiple_completions),
            user("tail"),
        ];
        let keep_mixed = pair_safe_cut_index(&messages, 2);
        assert_eq!(keep_mixed, 3);
        assert_eq!(completion_entries_before_cut(&messages, keep_mixed), 1);
        let drop_mixed = pair_safe_cut_index(&messages, 1);
        assert_eq!(drop_mixed, 4);
        assert_eq!(completion_entries_before_cut(&messages, drop_mixed), 2);
        assert_eq!(completion_entries_before_cut(&messages, 0), 0);
    }

    /// Only completion entries before the cut count; ones in the kept window
    /// and ordinary notices do not.
    #[test]
    fn dropped_completion_entries_count_only_rows_before_the_cut() {
        let messages = vec![
            completion_entry("job-a"),
            user("a"),
            Message::SystemNotice(SystemNoticeMessage::new(
                SystemNoticeKind::Generic,
                "Background fork_off job job-x finished",
            )),
            completion_entry("job-b"),
            user("b"),
            completion_entry("job-c"),
            user("tail"),
        ];
        let cut = pair_safe_cut_index(&messages, 3);
        assert_eq!(cut, 4);
        assert_eq!(completion_entries_before_cut(&messages, cut), 2);
        assert_eq!(completion_entries_before_cut(&messages, 0), 0);
        assert_eq!(completion_entries_before_cut(&messages, messages.len()), 3);
    }

    #[test]
    fn cut_lands_on_plain_message_boundary() {
        let messages = vec![user("a"), user("b"), user("c"), user("d")];
        assert_eq!(pair_safe_cut_index(&messages, 2), 2);
    }

    /// The naive cut would land ON the tool_results (index 2), orphaning it
    /// from its assistant tool-use message at index 1. The pair-safe cut must
    /// walk back to include the whole pair.
    #[test]
    fn cut_never_orphans_tool_results_from_their_assistant_message() {
        let (assistant, results) = tool_use_pair("call-1");
        let messages = vec![user("a"), assistant, results, user("tail")];
        assert_eq!(pair_safe_cut_index(&messages, 2), 1);
    }

    #[test]
    fn cut_after_tool_pair_is_untouched() {
        let (assistant, results) = tool_use_pair("call-1");
        let messages = vec![user("a"), assistant, results, user("tail")];
        // keep_last = 1 cuts [0, 3): the pair is entirely inside the cut.
        assert_eq!(pair_safe_cut_index(&messages, 1), 3);
    }

    #[test]
    fn cut_walks_back_to_zero_when_transcript_leads_with_pairs() {
        let (assistant, results) = tool_use_pair("call-1");
        let messages = vec![assistant, results, user("tail")];
        // Naive cut = 1 lands on the results; walking back reaches 0.
        assert_eq!(pair_safe_cut_index(&messages, 2), 0);
    }

    use crate::identity_first::{
        AgentAddressability, DurabilityPolicy, DurableAgentSpec, IdentityRuntime,
        IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
        RosterContext, RosterError, RosterProvider,
    };
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn worker_spec(identity: &AgentIdentity) -> DurableAgentSpec {
        DurableAgentSpec {
            identity: identity.clone(),
            profile: meerkat_mob::ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: std::collections::BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }
    }

    struct EmptyRoster;

    #[async_trait]
    impl RosterProvider for EmptyRoster {
        async fn roster(
            &self,
            _context: &RosterContext,
        ) -> Result<Vec<DurableAgentSpec>, RosterError> {
            Ok(Vec::new())
        }
    }

    /// Scripted LLM double reporting REAL input-token usage (zero usage
    /// silently disarms the compaction input trigger) with an optional gate
    /// that holds a turn in flight so the Busy refusal is deterministic.
    struct GatedUsageLlmClient {
        input_tokens: u64,
        gate_armed: Arc<AtomicBool>,
        in_call: Arc<AtomicBool>,
        /// Fired (one stored permit) when a gated call parks at the gate: a
        /// typed "the turn is mid-LLM-call" signal for tests that need it.
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        maintenance_only: Arc<AtomicBool>,
        fail_maintenance: Arc<AtomicBool>,
    }

    impl meerkat_client::LlmClient for GatedUsageLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }
        fn stream<'a>(
            &'a self,
            request: &'a meerkat_client::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>,
                    > + Send
                    + 'a,
            >,
        > {
            use futures::StreamExt;
            let maintenance = serde_json::to_string(&request.messages)
                .expect("test request")
                .contains("[meta-gateway operator verb compact_member]");
            let gate_armed = self.gate_armed.load(Ordering::SeqCst)
                && (!self.maintenance_only.load(Ordering::SeqCst) || maintenance);
            let fail = maintenance && self.fail_maintenance.load(Ordering::SeqCst);
            let in_call = Arc::clone(&self.in_call);
            let entered = Arc::clone(&self.entered);
            let release = Arc::clone(&self.release);
            let input_tokens = self.input_tokens;
            Box::pin(
                futures::stream::once(async move {
                    if gate_armed {
                        // Register for the release BEFORE announcing entry, so
                        // a release sent right after `entered` cannot be lost.
                        let released = release.notified();
                        tokio::pin!(released);
                        released.as_mut().enable();
                        in_call.store(true, Ordering::SeqCst);
                        entered.notify_one();
                        released.await;
                    }
                    in_call.store(false, Ordering::SeqCst);
                    if fail {
                        return futures::stream::iter(vec![Err(
                            meerkat_client::LlmError::InvalidRequest {
                                message: "injected terminal maintenance failure".to_string(),
                            },
                        )]);
                    }
                    let [usage, done] =
                        crate::mob_handle_runtime::test_llm_usage::usage_then_done_with(
                            request,
                            meerkat_core::Provider::OpenAI,
                            meerkat_core::types::Usage {
                                input_tokens,
                                ..Default::default()
                            },
                            meerkat_core::types::StopReason::EndTurn,
                        );
                    futures::stream::iter(vec![
                        Ok(meerkat_client::LlmEvent::TextDelta {
                            delta: "ack".to_string(),
                            meta: None,
                        }),
                        Ok(usage),
                        Ok(done),
                    ])
                })
                .flatten(),
            )
        }
        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }
        fn health_check<'life0, 'async_trait>(
            &'life0 self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<(), meerkat_client::LlmError>>
                    + Send
                    + 'async_trait,
            >,
        >
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async { Ok(()) })
        }
    }

    struct OperatorVerbHarness {
        _temp_dir: tempfile::TempDir,
        runtime: crate::UnifiedRuntime,
        concrete: Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
        adapter: Arc<meerkat_runtime::MeerkatMachine>,
        identity_runtime: Arc<IdentityRuntime>,
        floors: Arc<crate::identity_first::CompactionFloorRegistry>,
        identity: AgentIdentity,
        member_alias: String,
        gate_armed: Arc<AtomicBool>,
        in_call: Arc<AtomicBool>,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        maintenance_only: Arc<AtomicBool>,
        fail_maintenance: Arc<AtomicBool>,
    }

    impl OperatorVerbHarness {
        fn identity_ctx(&self) -> IdentityFirstContext {
            IdentityFirstContext {
                runtime: crate::identity_first::PublicIdentityRuntime::new(
                    Arc::clone(&self.identity_runtime),
                    self.runtime.mob_runtime().clone(),
                ),
                roster_provider: Arc::new(EmptyRoster),
                topology_provider: None,
                customizer: None,
                agent_memory_provider: None,
                mob_definition: Some(self.runtime.mob_handle().definition().clone()),
                transcript_edit_service: Some(Arc::clone(&self.concrete) as _),
                compaction_floors: Some(Arc::clone(&self.floors)),
            }
        }

        /// Drive one member turn to completion through the identity runtime.
        /// Stop the mob the way production teardown does. A member that is
        /// still runtime-attached (mid-kickoff after a restore, or a turn
        /// admitted a moment ago) refuses a raw `MobHandle::stop` with
        /// `Runtime not ready: attached`; the unified runtime waits that
        /// readiness window out and degrades it to a typed outcome instead of
        /// failing teardown. Every other refusal still fails the test.
        async fn teardown(&self) {
            match self.runtime.stop_mob_for_teardown().await {
                crate::unified_runtime::MobStopOutcome::Stopped
                | crate::unified_runtime::MobStopOutcome::ProceededWithoutInterrupt { .. } => {}
                crate::unified_runtime::MobStopOutcome::Failed(error) => {
                    panic!("mob teardown failed: {error}");
                }
            }
        }

        async fn run_turn(&self, text: String) {
            let admission = self
                .identity_runtime
                .send_admission_tracked(
                    &self.identity,
                    None,
                    &meerkat_core::ContentInput::Text(text),
                    meerkat_core::types::HandlingMode::Queue,
                    None,
                )
                .await
                .expect("seed turn admitted");
            self.identity_runtime
                .wait_for_completion(
                    &self.identity,
                    admission.completion_baseline,
                    Duration::from_secs(30),
                )
                .await
                .expect("seed turn completed");
        }

        /// Observe the exact probe's finalized receipt before tearing down the
        /// runtime; a session-wide completion cursor can precede finalization.
        async fn run_exact_probe(&self, text: &str) {
            use meerkat_runtime::SessionServiceRuntimeExt;
            let owner = meerkat_mob::MobSessionService::acquire_runtime_adapter(
                self.concrete.as_ref(),
                None,
            )
            .expect("acquire the operator fixture runtime owner")
            .expect("persistent completion owner");
            let session = self
                .identity_runtime
                .status(&self.identity)
                .await
                .expect("probe identity")
                .session_id
                .expect("probe session");
            let correlation = meerkat_core::SessionId::new().to_string();
            let key = format!("operator-probe:{correlation}");
            self.identity_runtime
                .dispatch_admission_tracked(
                    &self.identity,
                    None,
                    &crate::identity_first::DispatchInput::system(text)
                        .with_correlation(correlation)
                        .with_idempotency(&key),
                )
                .await
                .expect("exact probe admitted");
            tokio::time::timeout(Duration::from_secs(30), async {
                let mut input_id = None;
                loop {
                    if input_id.is_none() {
                        input_id = owner
                            .input_state_by_idempotency_key(&session, &key)
                            .await
                            .expect("probe input read")
                            .map(|state| state.state.input_id);
                    }
                    if let Some(id) = &input_id
                        && let Some(outcome) = owner
                            .input_terminal_completion(&session, id)
                            .await
                            .expect("probe terminal read")
                    {
                        match outcome {
                            meerkat_runtime::CompletionOutcome::Completed(result) => {
                                assert_eq!(result.session_id, session);
                                break;
                            }
                            other => panic!("probe did not complete successfully: {other:?}"),
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("exact probe finalizes within backstop");
        }

        async fn transcript_facts(&self) -> (usize, Option<String>) {
            let service: Arc<dyn crate::memory::hygienist::TranscriptEditSessionService> =
                Arc::clone(&self.concrete) as _;
            let session_id = self
                .identity_runtime
                .status(&self.identity)
                .await
                .expect("identity status")
                .session_id
                .expect("identity session");
            let (count, _head, last_reason) = read_transcript_facts(&service, &session_id)
                .await
                .expect("transcript facts");
            (count, last_reason)
        }

        /// `transcript_facts` count, read off a transcript that has stopped
        /// moving. Use this for any count a later assertion is COMPARED
        /// AGAINST: a baseline snapshotted mid-write drifts upward on its own,
        /// and then a "did it grow" check passes without the growth it names.
        async fn settled_transcript_count(&self) -> usize {
            let service: Arc<dyn crate::memory::hygienist::TranscriptEditSessionService> =
                Arc::clone(&self.concrete) as _;
            let session_id = self
                .identity_runtime
                .status(&self.identity)
                .await
                .expect("identity status")
                .session_id
                .expect("identity session");
            settled_transcript_len(&service, &session_id).await
        }
    }

    /// Message count on the DURABLE session surface - the one the operator
    /// verbs read, which is not the same clock as the identity's completion
    /// cursor.
    async fn transcript_len(
        service: &Arc<dyn crate::memory::hygienist::TranscriptEditSessionService>,
        session_id: &meerkat_core::types::SessionId,
    ) -> usize {
        service
            .read_history(
                session_id,
                meerkat_core::service::SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("read durable transcript")
            .messages
            .len()
    }

    /// Wait until a turn seeded with `user_text` is durable end to end: its
    /// input row (user row or queue-mode system notice) is in the durable
    /// transcript and the transcript ends on an assistant row. A turn's completion cursor advances before the turn's
    /// rows are durable in the session store, and under full-suite contention
    /// that lag exceeded the stability window `settled_transcript_len` uses
    /// (the seed turn's two rows landed after three stable 100 ms reads, so
    /// the fixture seeded below them and every derived index was off by two).
    /// Naming the rows that must exist turns the wait into a real condition.
    async fn wait_for_durable_turn(
        service: &Arc<dyn crate::memory::hygienist::TranscriptEditSessionService>,
        session_id: &meerkat_core::types::SessionId,
        user_text: &str,
    ) {
        crate::test_wait::poll_until(
            &format!("turn {user_text:?} is durable (user row present, assistant row last)"),
            crate::test_wait::STRUCTURAL_BACKSTOP,
            async || {
                let page = service
                    .read_history(
                        session_id,
                        meerkat_core::service::SessionHistoryQuery {
                            offset: 0,
                            limit: None,
                        },
                    )
                    .await
                    .expect("read durable transcript");
                // Queue-mode text through the identity runtime lands as a
                // system notice row; a direct user turn lands as a user row.
                // Either is the turn's input row.
                let has_input = page.messages.iter().any(|message| match message {
                    Message::User(user) => user.content.iter().any(|block| {
                        matches!(block, meerkat_core::types::ContentBlock::Text { text } if text == user_text)
                    }),
                    Message::SystemNotice(notice) => notice
                        .body
                        .as_deref()
                        .is_some_and(|body| body.contains(user_text)),
                    _ => false,
                });
                has_input && matches!(page.messages.last(), Some(Message::BlockAssistant(_)))
            },
        )
        .await;
    }

    /// [`transcript_len`], but only once the durable transcript has stopped
    /// moving.
    ///
    /// A turn's completion cursor advances before the turn's rows are all
    /// durable, so a length read immediately after `run_turn` is a RACING
    /// READ. Any index computed from it is stale the moment a trailing row
    /// lands, which does not happen on an idle box and does happen under
    /// full-suite contention. Requiring the length to repeat across
    /// consecutive observations is what makes it usable as an index.
    ///
    /// The ceiling is a backstop, not a measurement: the transcript settles in
    /// milliseconds when anything is working.
    async fn settled_transcript_len(
        service: &Arc<dyn crate::memory::hygienist::TranscriptEditSessionService>,
        session_id: &meerkat_core::types::SessionId,
    ) -> usize {
        const REQUIRED_STABLE_OBSERVATIONS: usize = 3;
        let deadline = std::time::Instant::now() + Duration::from_mins(1);
        let mut settled = transcript_len(service, session_id).await;
        let mut stable = 1;
        while stable < REQUIRED_STABLE_OBSERVATIONS {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let observed = transcript_len(service, session_id).await;
            if observed == settled {
                stable += 1;
            } else {
                settled = observed;
                stable = 1;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "durable transcript never settled: still growing (last observed {observed} \
                 messages), so no index derived from it can be trusted"
            );
        }
        settled
    }

    /// Full production wiring in one process: a concrete
    /// `PersistentSessionService` backing the mob, an identity-first member
    /// bridged by the production `MobSessionBridge`, and the bridge's own
    /// compaction-floor registry shared into the RPC context - exactly the
    /// rpc_gateway composition, minus the stdio callback bridge.
    async fn operator_verb_harness(member: &str, mob_id: &str) -> OperatorVerbHarness {
        operator_verb_harness_with_fork_policy(
            member,
            mob_id,
            meerkat_core::ForkInstructionPolicy::default(),
        )
        .await
    }

    /// [`operator_verb_harness`] with the host's fork instruction policy for
    /// the mob's member forks.
    async fn operator_verb_harness_with_fork_policy(
        member: &str,
        mob_id: &str,
        fork_instruction_policy: meerkat_core::ForkInstructionPolicy,
    ) -> OperatorVerbHarness {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let state = temp_dir.path().join("state");
        std::fs::create_dir_all(&state).expect("state dir");
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
            meerkat_store::SqliteSessionStore::open(state.join("sessions.db"))
                .expect("session store"),
        );
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))
                .expect("runtime store"),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let factory = meerkat::AgentFactory::new(&state).comms(true);
        let mut inner_builder =
            meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
        inner_builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        inner_builder.default_blob_store = Some(blob_store.clone());
        let adapter = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(
                Arc::clone(&runtime_store),
                Arc::clone(&blob_store),
            )
            .expect("runtime owner"),
        );
        let concrete = Arc::new(
            meerkat_session::PersistentSessionService::new(
                inner_builder,
                16,
                session_store.clone(),
                runtime_store,
                blob_store,
            )
            .with_canonical_runtime_adapter(adapter.clone()),
        );

        let gate_armed = Arc::new(AtomicBool::new(false));
        let in_call = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let maintenance_only = Arc::new(AtomicBool::new(false));
        let fail_maintenance = Arc::new(AtomicBool::new(false));
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"

[profiles.worker.tools]
comms = true
"#
        ))
        .expect("mob definition");
        let mut mob_spec = crate::mob_handle_runtime::MobBootstrapSpec::new(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            concrete.clone(),
        )
        .with_session_runtime_adapter(Arc::clone(&adapter))
        .expect("acquire the fixture session runtime owner")
        .with_fork_instruction_policy(fork_instruction_policy)
        .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(GatedUsageLlmClient {
                input_tokens: 5_000,
                gate_armed: Arc::clone(&gate_armed),
                in_call: Arc::clone(&in_call),
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
                maintenance_only: Arc::clone(&maintenance_only),
                fail_maintenance: Arc::clone(&fail_maintenance),
            })),
        });
        mob_spec.committed_boundary_recoverer = Some(concrete.clone());
        let mut runtime = crate::UnifiedRuntime::bootstrap(
            mob_spec,
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: mob_id.to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            Duration::from_secs(5),
        )
        .await
        .expect("bootstrap unified runtime");
        let handle = runtime.mob_handle();

        // Identity-first member: the roster id is the mk--encoded generated
        // runtime alias, the shape downstream app fleets address. The
        // `agent_identity` label is what the identity health monitor uses to
        // map this member's RunCompleted events onto the durable identity's
        // completion cursor.
        // `member` is the DURABLE identity now, not a generated `rt:{id}:{gen}`
        // alias: the roster identity is the durable identity's comms-safe
        // encoding, and AgentRuntimeId is incarnation detail. This used to
        // receive an alias and derive the durable identity back out of it.
        let roster_id = crate::member_comms_id::mob_member_id_str(member).into_owned();
        let roster_identity = meerkat_mob::ids::AgentIdentity::from(roster_id.clone());
        let durable_identity = crate::identity_first::AgentIdentity::parse(member)
            .expect("member argument must be a durable identity");
        let mut member_labels = std::collections::BTreeMap::new();
        member_labels.insert(
            "agent_identity".to_string(),
            durable_identity.as_str().to_string(),
        );
        handle
            .ensure_member(
                meerkat_mob::SpawnMemberSpec::new(
                    meerkat_mob::ProfileName::from("worker"),
                    roster_identity.clone(),
                )
                .with_labels(member_labels),
            )
            .await
            .expect("spawn identity-first member");
        handle
            .wait_for_members_kickoff_complete(
                std::slice::from_ref(&roster_identity),
                // A settle bound only: the assertions below check state, not elapsed time.
                Some(Duration::from_secs(30)),
            )
            .await
            .expect("member kickoff settled");
        // Durable identity authority over that member, bridged by the
        // production MobSessionBridge (the gateway wiring). Core's journal
        // binds the member spawned above; the identity runtime reads its
        // status, runtime id and session from there.
        let public_member_alias =
            crate::member_comms_id::runtime_alias_str(&roster_id).into_owned();
        let identity = durable_identity;
        let continuity_store =
            Arc::new(LocalContinuityStore::in_memory().expect("continuity store"));
        let lease_provider = Arc::new(LocalLeaseProvider::new());
        let bridge = MobSessionBridge::with_session_service(handle.clone(), concrete.clone());
        let floors = bridge.compaction_floors();
        let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store,
            lease_provider,
            runtime_instance_id: "operator-verb-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(bridge)),
            default_timeout: None,
        }));
        identity_runtime.register(worker_spec(&identity)).await;

        // Install the identity authority on the unified runtime so its
        // identity health monitor drives the completion cursor from member
        // RunCompleted events (`wait_for_completion` moves on nothing else).
        runtime.attach_identity_first_context(Arc::new(
            crate::identity_first::IdentityFirstRuntimeContext::new(
                Arc::clone(&identity_runtime),
                Arc::new(EmptyRoster),
                None,
                None,
                Some(handle.definition().clone()),
            ),
        ));

        OperatorVerbHarness {
            _temp_dir: temp_dir,
            runtime,
            concrete,
            adapter,
            identity_runtime,
            floors,
            identity,
            member_alias: public_member_alias,
            gate_armed,
            in_call,
            entered,
            release,
            maintenance_only,
            fail_maintenance,
        }
    }

    async fn rpc(harness: &OperatorVerbHarness, method: &str, params: Value) -> Value {
        let ctx = harness.identity_ctx();
        let raw = handle_unified_rpc_json(
            &harness.runtime,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            })
            .to_string(),
            Duration::from_mins(1),
            None,
            Some(&ctx),
        )
        .await;
        serde_json::from_str(&raw).expect("json-rpc response")
    }

    /// End-to-end `meta/compact_member`: seed a member transcript past the
    /// floor, invoke the verb, and require (a) a compaction-semantic rewrite
    /// landed (messages_after < messages_before), (b) the floor registry is
    /// disarmed afterwards, and (c) the restored build does NOT keep
    /// compacting (the compact-every-turn hazard the restore step exists to
    /// prevent).
    ///
    /// What this proves with the scripted keyless LLM double: the full arm ->
    /// respawn -> forced pre-LLM compaction -> disarm -> restore-respawn loop
    /// over the production bridge/service wiring, with the double reporting
    /// real input-token usage (5k > the 256-token floor) so the provider
    /// input trigger fires exactly as it would in production. It does NOT
    /// prove real-model summary quality.
    #[tokio::test(flavor = "multi_thread")]
    async fn compact_member_forces_one_compaction_and_restores_the_profile() {
        let harness = operator_verb_harness("worker:main", "operator-compact-verb").await;

        // Seed past both the floor and the recent-turn budget (4 turns) so
        // compaction has an older region to summarize away.
        let fat = "seeded transcript ballast ".repeat(160);
        for turn in 0..8 {
            harness.run_turn(format!("turn {turn}: {fat}")).await;
        }
        // Settled: this is a LOWER BOUND on a durable surface that lags the
        // completion cursor, so an unsettled read fails at `>= 16` for a
        // transcript that is merely still landing rather than one that is short.
        let messages_before = harness.settled_transcript_count().await;
        assert!(
            messages_before >= 16,
            "seed must materialize a fat transcript, got {messages_before}"
        );

        let response = rpc(
            &harness,
            "meta/compact_member",
            serde_json::json!({
                "identity": harness.member_alias,
                "floor_tokens": 256,
                "timeout_ms": 30_000,
            }),
        )
        .await;
        assert!(
            response["error"].is_null(),
            "compact_member must succeed: {response:#?}"
        );
        let result = &response["result"];
        assert_eq!(
            result["compaction_applied"],
            Value::Bool(true),
            "{result:#?}"
        );
        let reported_before = result["messages_before"].as_u64().expect("messages_before");
        let reported_after = result["messages_after"].as_u64().expect("messages_after");
        assert!(
            reported_after < reported_before,
            "compaction must shrink the transcript: {result:#?}"
        );
        assert!(
            result["last_rewrite_reason"]
                .as_str()
                .is_some_and(|reason| reason.to_lowercase().contains("compact")),
            "the landed rewrite must be compaction-semantic: {result:#?}"
        );
        assert!(
            harness.floors.get(&harness.identity).is_none(),
            "the floor registry must be disarmed after the verb"
        );

        // Restore evidence: the post-verb build must be back on the original
        // profile threshold. The recent turns alone (~5k reported input
        // tokens) are far past the 256-token floor, so a still-armed floor
        // would compact again on this very turn.
        // Both reads are on the DURABLE session surface, and `run_turn` returns
        // on the identity's COMPLETION CURSOR, which leads it (same clock split
        // documented on `transcript_len` and already fixed once in the sibling
        // timeout test). Sampling the surface once, at cursor timing, made this
        // read the append before it landed and report `3 -> 3` - a lagging
        // write and a wrongly re-compacting build are indistinguishable in a
        // single sample. Baseline is settled so it cannot drift upward on its
        // own and satisfy the comparison without an append; the growth itself
        // is then polled under the structural backstop, so a build that really
        // does re-compact never satisfies it and fails naming what never
        // happened.
        let count_after_verb = harness.settled_transcript_count().await;
        harness.run_turn("post-verb probe turn".to_string()).await;
        crate::test_wait::poll_until(
            &format!(
                "the restored build appended to the durable transcript instead of re-compacting \
                 (still {count_after_verb} messages)"
            ),
            crate::test_wait::STRUCTURAL_BACKSTOP,
            async || harness.transcript_facts().await.0 > count_after_verb,
        )
        .await;

        // Authorization shape: an identity the gateway does not own is a
        // typed refusal, not a fall-through.
        let response = rpc(
            &harness,
            "meta/compact_member",
            serde_json::json!({ "identity": "nobody:here" }),
        )
        .await;
        assert_eq!(
            response["error"]["code"],
            serde_json::json!(-32001),
            "an unowned identity must surface the typed unknown-identity refusal: {response:#?}"
        );

        harness.teardown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn autonomous_compaction_exact_input_completion_survives_observation_timeout() {
        use meerkat_runtime::SessionServiceRuntimeExt;

        let harness = operator_verb_harness("worker:main", "operator-exact-input").await;
        let completion_runtime = meerkat_mob::MobSessionService::acquire_runtime_adapter(
            harness.concrete.as_ref(),
            None,
        )
        .expect("acquire the operator fixture runtime owner")
        .expect("persistent service runtime adapter");
        assert!(completion_runtime.shares_runtime_execution_owner_with(&harness.adapter));
        let configured_observer = harness
            .runtime
            .mob_runtime()
            .committed_boundary_recoverer()
            .expect("concrete owner")
            .runtime_completion_observer()
            .expect("owner observer");
        assert!(
            configured_observer.shares_runtime_execution_owner_with(&completion_runtime),
            "operator observer must use the concrete service's acquired runtime owner"
        );
        let fat = "seeded transcript ballast ".repeat(160);
        for turn in 0..4 {
            harness.run_turn(format!("turn {turn}: {fat}")).await;
        }
        let ctx = harness.identity_ctx();
        let spec = harness
            .identity_runtime
            .roster_inspect()
            .await
            .expect("roster inspect")
            .remove(&harness.identity)
            .expect("registered spec")
            .0;
        harness.floors.set(
            &harness.identity,
            NonZeroU64::new(256).expect("positive floor"),
        );
        let session_id =
            rebuild_member_for_fresh_build(&ctx, &harness.identity, None, spec.clone())
                .await
                .expect("install floor");
        let member_id = meerkat_mob::AgentIdentity::from(
            crate::member_comms_id::mob_member_id_str(harness.identity.as_str()).into_owned(),
        );
        let handle = harness.runtime.mob_handle();
        let member = handle
            .get_member(&member_id)
            .await
            .expect("read member")
            .expect("member present");
        assert_eq!(
            member.runtime_mode,
            meerkat_mob::MobRuntimeMode::AutonomousHost
        );
        let runtime_id = member.agent_runtime_id.clone();
        let fence = member.fence_token;
        let correlation = meerkat_core::SessionId::new().to_string();
        let key = format!("meta-compact-proof:{correlation}");
        let delivery = meerkat_mob::store::MobDeliveryIdentity::new(&key, &correlation)
            .expect("caller-owned delivery identity");
        harness.gate_armed.store(true, Ordering::SeqCst);
        handle
            .submit_work_with_mode_and_delivery_identity(
                runtime_id.clone(),
                fence,
                meerkat_mob::WorkSpec::new(
                    "exact input compaction maintenance",
                    meerkat_mob::WorkOrigin::Internal,
                ),
                meerkat_core::types::HandlingMode::Queue,
                delivery,
            )
            .await
            .expect("one autonomous ingress admission");
        let input = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(input) = completion_runtime
                    .input_state_by_idempotency_key(&session_id, &key)
                    .await
                    .expect("owner admission read")
                {
                    break input.state.input_id;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("exact key resolves within backstop");
        assert!(
            completion_runtime
                .input_state_by_idempotency_key(&session_id, "unrelated-operation")
                .await
                .expect("unrelated key read")
                .is_none()
        );
        let completion = async {
            loop {
                if let Some(completion) = completion_runtime
                    .input_terminal_completion(&session_id, &input)
                    .await
                    .expect("exact rich terminal read")
                {
                    break completion;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        tokio::pin!(completion);
        assert!(
            tokio::time::timeout(Duration::from_millis(1), &mut completion)
                .await
                .is_err(),
            "admission alone cannot complete a held input"
        );
        harness.gate_armed.store(false, Ordering::SeqCst);
        harness.release.notify_one();
        let outcome = tokio::time::timeout(Duration::from_secs(30), &mut completion).await;
        if outcome.is_err() {
            let state = completion_runtime
                .input_state(&session_id, &input)
                .await
                .expect("exact input diagnostic");
            eprintln!(
                "exact input timeout: key={key} input={input:?} state={state:?} in_call={}",
                harness.in_call.load(Ordering::SeqCst),
            );
        }
        let outcome = outcome.expect("original input completion within backstop");
        match outcome {
            meerkat_runtime::CompletionOutcome::Completed(result) => {
                assert_eq!(result.session_id, session_id);
            }
            other => panic!("expected exact successful terminal, got {other:?}"),
        }
        harness.floors.clear(&harness.identity);
        let restored = rebuild_member_for_fresh_build(&ctx, &harness.identity, None, spec)
            .await
            .expect("restore only after rich exact input completion");
        assert_eq!(restored, session_id);
        let before = harness.settled_transcript_count().await;
        harness.run_turn("post-exact-input probe".to_string()).await;
        crate::test_wait::poll_until(
            "member accepts durable work after exact input settlement",
            crate::test_wait::STRUCTURAL_BACKSTOP,
            async || harness.transcript_facts().await.0 > before,
        )
        .await;
        harness.teardown().await;
    }

    async fn held_compaction_harness(name: &str) -> Arc<OperatorVerbHarness> {
        let harness = Arc::new(operator_verb_harness("worker:main", name).await);
        let fat = "seeded transcript ballast ".repeat(160);
        for turn in 0..4 {
            harness.run_turn(format!("turn {turn}: {fat}")).await;
        }
        harness.maintenance_only.store(true, Ordering::SeqCst);
        harness.gate_armed.store(true, Ordering::SeqCst);
        harness
    }

    fn start_held_compaction(harness: &Arc<OperatorVerbHarness>) -> tokio::task::JoinHandle<Value> {
        let harness = Arc::clone(harness);
        tokio::spawn(async move {
            rpc(
                &harness,
                "meta/compact_member",
                serde_json::json!({
                    "identity": harness.member_alias,
                    "floor_tokens": 256,
                    "timeout_ms": 1,
                }),
            )
            .await
        })
    }

    async fn wait_for_held_compaction(harness: &OperatorVerbHarness) {
        crate::test_wait::poll_until(
            "maintenance LLM holds the admitted compaction",
            crate::test_wait::STRUCTURAL_BACKSTOP,
            async || harness.in_call.load(Ordering::SeqCst),
        )
        .await;
    }

    async fn release_compaction_and_join(harness: &OperatorVerbHarness) {
        harness.gate_armed.store(false, Ordering::SeqCst);
        harness.release.notify_one();
        tokio::time::timeout(
            Duration::from_secs(30),
            harness.identity_runtime.join_foreground_operations(),
        )
        .await
        .expect("owned compaction cleanup finishes");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_member_secondary_timeout_retains_owner_without_retire_or_second_send() {
        let harness = held_compaction_harness("operator-secondary-timeout").await;
        let caller = start_held_compaction(&harness);
        wait_for_held_compaction(&harness).await;
        let before = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("status");
        let response = tokio::time::timeout(Duration::from_secs(15), caller)
            .await
            .expect("secondary observation is bounded")
            .expect("caller task");
        assert_eq!(response["error"]["data"]["kind"], "compact_member_timeout");
        assert_eq!(response["error"]["data"]["stage"], "terminal_pending");
        assert!(response["result"].is_null());
        assert!(harness.floors.get(&harness.identity).is_some());
        assert!(harness.in_call.load(Ordering::SeqCst));
        let after = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("status");
        assert_eq!(
            after.state,
            crate::identity_first::IdentityLifecycleState::Active
        );
        assert_eq!(after.session_id, before.session_id);
        assert_eq!(after.agent_runtime_id, before.agent_runtime_id);
        let concurrent = rpc(
            &harness,
            "meta/compact_member",
            serde_json::json!({
                "identity": harness.member_alias, "timeout_ms": 1,
            }),
        )
        .await;
        assert_eq!(concurrent["error"]["data"]["kind"], "compact_member_busy");
        release_compaction_and_join(&harness).await;
        assert!(harness.floors.get(&harness.identity).is_none());
        let settled = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("settled");
        assert_eq!(
            settled.state,
            crate::identity_first::IdentityLifecycleState::Active
        );
        assert_eq!(settled.session_id, before.session_id);
        harness
            .run_exact_probe("after pending-owner settlement")
            .await;
        harness.teardown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_member_caller_drop_keeps_exact_completion_and_profile_cleanup_owned() {
        let harness = held_compaction_harness("operator-caller-drop").await;
        let caller = start_held_compaction(&harness);
        wait_for_held_compaction(&harness).await;
        let original = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("status");
        caller.abort();
        assert!(caller.await.expect_err("caller cancelled").is_cancelled());
        assert!(harness.floors.get(&harness.identity).is_some());
        release_compaction_and_join(&harness).await;
        assert!(harness.floors.get(&harness.identity).is_none());
        let restored = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("restored");
        assert_eq!(restored.session_id, original.session_id);
        assert_eq!(
            restored.state,
            crate::identity_first::IdentityLifecycleState::Active
        );
        harness.run_exact_probe("after caller dropped").await;
        harness.teardown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_member_failed_exact_terminal_does_not_authorize_rebuild() {
        let harness = held_compaction_harness("operator-terminal-failure").await;
        harness.fail_maintenance.store(true, Ordering::SeqCst);
        let caller = start_held_compaction(&harness);
        wait_for_held_compaction(&harness).await;
        let before = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("status");
        release_compaction_and_join(&harness).await;
        let response = caller.await.expect("caller result");
        assert_eq!(
            response["error"]["data"]["kind"],
            "compact_member_completion_failed"
        );
        assert!(
            response["error"]["data"]["completion_type"]
                .as_str()
                .is_some()
        );
        assert!(response["result"].is_null());
        assert!(harness.floors.get(&harness.identity).is_some());
        let after = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("status");
        assert_eq!(after.session_id, before.session_id);
        assert_eq!(after.agent_runtime_id, before.agent_runtime_id);
        harness.teardown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_member_superseded_incarnation_cannot_restore_or_revive_retired_member() {
        let harness = held_compaction_harness("operator-superseded").await;
        let caller = start_held_compaction(&harness);
        wait_for_held_compaction(&harness).await;
        harness
            .identity_runtime
            .retire_tracked(&harness.identity)
            .await
            .expect("explicit retire");
        let retired = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("retired");
        release_compaction_and_join(&harness).await;
        let response = caller.await.expect("caller");
        assert_eq!(
            response["error"]["data"]["kind"],
            "compact_member_superseded"
        );
        let after = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("status");
        // Core retired the member and nothing revived it: Dormant (there is
        // no Retiring state; a retired identity is Dormant until its next
        // wake).
        assert_eq!(
            after.state,
            crate::identity_first::IdentityLifecycleState::Dormant
        );
        assert!(harness.floors.get(&harness.identity).is_none());
        assert_eq!(after.session_id, retired.session_id);
        assert_eq!(after.agent_runtime_id, retired.agent_runtime_id);
        harness.teardown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_member_stale_pre_admission_owner_cannot_revive_retired_target() {
        let harness = operator_verb_harness("worker:main", "operator-stale-before-admit").await;
        let expected = harness
            .identity_runtime
            .capture_incarnation(&harness.identity)
            .await
            .expect("capture owner");
        harness
            .identity_runtime
            .retire_tracked(&harness.identity)
            .await
            .expect("retire");
        let input = crate::identity_first::DispatchInput::system("must never be admitted")
            .with_idempotency("stale-maintenance")
            .with_correlation(meerkat_core::SessionId::new().to_string());
        let result = harness
            .identity_runtime
            .dispatch_with_expected_incarnation(&harness.identity, None, Some(&expected), &input)
            .await;
        assert!(matches!(
            result,
            Err(crate::identity_first::IdentityRuntimeError::PostAdmissionSuperseded { .. })
        ));
        let after = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("status");
        // Core retired the member and nothing revived it: Dormant (there is
        // no Retiring state; a retired identity is Dormant until its next
        // wake).
        assert_eq!(
            after.state,
            crate::identity_first::IdentityLifecycleState::Dormant
        );
        assert!(harness.floors.get(&harness.identity).is_none());
        harness.teardown().await;
    }

    struct FailingCompletionObserver {
        inner: Arc<meerkat_runtime::MeerkatMachine>,
        failing: AtomicBool,
    }

    struct AdmissionReplyLost {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl CompactAdmission for AdmissionReplyLost {
        async fn dispatch(
            &self,
            runtime: &crate::identity_first::IdentityRuntime,
            identity: &AgentIdentity,
            incarnation: &crate::identity_first::runtime::CapturedIncarnation,
            input: &crate::identity_first::DispatchInput,
        ) -> Result<
            crate::identity_first::runtime::DispatchOutcome,
            crate::identity_first::IdentityRuntimeError,
        > {
            self.calls.fetch_add(1, Ordering::SeqCst);
            IdentityCompactionAdmission
                .dispatch(runtime, identity, incarnation, input)
                .await?;
            Err(
                crate::identity_first::IdentityRuntimeError::ActorAdmissionTimeout {
                    identity: identity.clone(),
                    operation: "test.admission_reply",
                    waited: Duration::from_millis(1),
                    command: None,
                },
            )
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_member_uncertain_admission_keeps_original_key_and_cleanup_without_resend() {
        use meerkat_runtime::SessionServiceRuntimeExt;
        let harness = held_compaction_harness("operator-admission-uncertain").await;
        let admission = Arc::new(AdmissionReplyLost {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let observer = meerkat_mob::MobSessionService::acquire_runtime_adapter(
            harness.concrete.as_ref(),
            None,
        )
        .expect("acquire the operator fixture runtime owner")
        .expect("persistent owner");
        let operation_id = meerkat_core::SessionId::new().to_string();
        let key = format!("meta-compact:{operation_id}");
        let operation = CompactOperation {
            ctx: harness.identity_ctx(),
            identity: harness.identity.clone(),
            expected_alias: None,
            observer: observer.clone(),
            admission: admission.clone(),
            floors: harness.floors.clone(),
            floor: NonZeroU64::new(256).expect("floor"),
            timeout_ms: 1,
            response_id: serde_json::json!(1),
            operation_id,
        };
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let runtime = harness.identity_runtime.clone();
        let owner = tokio::spawn(async move {
            runtime
                .run_tracked_foreground(async move { Ok(operation.run(sender).await) })
                .await
        });
        wait_for_held_compaction(&harness).await;
        let response = tokio::time::timeout(Duration::from_secs(15), receiver)
            .await
            .expect("bounded admission response")
            .expect("response");
        assert_eq!(
            response.error.expect("error").data.expect("data")["kind"],
            "compact_member_admission_pending"
        );
        assert!(!owner.is_finished());
        assert!(harness.floors.get(&harness.identity).is_some());
        let session = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("status")
            .session_id
            .expect("session");
        let original = observer
            .input_state_by_idempotency_key(&session, &key)
            .await
            .expect("input read")
            .expect("admitted original input")
            .state
            .input_id;
        release_compaction_and_join(&harness).await;
        let response = owner.await.expect("owner").expect("result");
        assert_eq!(
            response.error.expect("timeout error").data.expect("data")["stage"],
            "profile_restored"
        );
        assert_eq!(admission.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            observer
                .input_state_by_idempotency_key(&session, &key)
                .await
                .expect("same-key read")
                .expect("original input retained")
                .state
                .input_id,
            original,
        );
        assert!(harness.floors.get(&harness.identity).is_none());
        harness.run_exact_probe("after lost admission reply").await;
        harness.teardown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_member_preparation_read_failure_does_not_install_or_strand_floor() {
        let harness = operator_verb_harness("worker:main", "operator-preflight-read").await;
        let other = operator_verb_harness("worker:other", "operator-other-store").await;
        let before = harness
            .identity_runtime
            .capture_incarnation(&harness.identity)
            .await
            .expect("capture");
        let mut ctx = harness.identity_ctx();
        ctx.transcript_edit_service = Some(other.concrete.clone());
        let operation = CompactOperation {
            ctx,
            identity: harness.identity.clone(),
            expected_alias: None,
            observer: meerkat_mob::MobSessionService::acquire_runtime_adapter(
                harness.concrete.as_ref(),
                None,
            )
            .expect("acquire the operator fixture runtime owner")
            .expect("observer"),
            admission: Arc::new(IdentityCompactionAdmission),
            floors: harness.floors.clone(),
            floor: NonZeroU64::new(256).expect("floor"),
            timeout_ms: 1,
            response_id: serde_json::json!(1),
            operation_id: meerkat_core::SessionId::new().to_string(),
        };
        let (sender, _receiver) = tokio::sync::oneshot::channel();
        let response = operation.run(sender).await;
        assert_eq!(
            response.error.expect("read error").data.expect("data")["stage"],
            "before"
        );
        assert!(harness.floors.get(&harness.identity).is_none());
        assert_eq!(
            harness
                .identity_runtime
                .capture_incarnation(&harness.identity)
                .await
                .expect("owner"),
            before,
        );
        harness.teardown().await;
        other.teardown().await;
    }

    #[async_trait::async_trait]
    impl CompactCompletionObserver for FailingCompletionObserver {
        async fn observe(
            &self,
            session_id: &meerkat_core::SessionId,
            key: &str,
            input_id: &mut Option<meerkat_core::lifecycle::InputId>,
        ) -> Result<Option<meerkat_runtime::CompletionOutcome>, meerkat_runtime::RuntimeDriverError>
        {
            if self.failing.load(Ordering::SeqCst) {
                return Err(meerkat_runtime::RuntimeDriverError::Internal(
                    "injected observation outage".to_string(),
                ));
            }
            CompactCompletionObserver::observe(self.inner.as_ref(), session_id, key, input_id).await
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compact_member_observation_error_retains_original_input_and_cleanup_owner() {
        let harness = held_compaction_harness("operator-read-error").await;
        let observer = Arc::new(FailingCompletionObserver {
            inner: meerkat_mob::MobSessionService::acquire_runtime_adapter(
                harness.concrete.as_ref(),
                None,
            )
            .expect("acquire the operator fixture runtime owner")
            .expect("persistent owner"),
            failing: AtomicBool::new(true),
        });
        let operation = CompactOperation {
            ctx: harness.identity_ctx(),
            identity: harness.identity.clone(),
            expected_alias: None,
            observer: observer.clone(),
            admission: Arc::new(IdentityCompactionAdmission),
            floors: harness.floors.clone(),
            floor: NonZeroU64::new(256).expect("floor"),
            timeout_ms: 1,
            response_id: serde_json::json!(1),
            operation_id: meerkat_core::SessionId::new().to_string(),
        };
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let runtime = harness.identity_runtime.clone();
        let owner = tokio::spawn(async move {
            runtime
                .run_tracked_foreground(async move { Ok(operation.run(sender).await) })
                .await
        });
        wait_for_held_compaction(&harness).await;
        let response = tokio::time::timeout(Duration::from_secs(15), receiver)
            .await
            .expect("bounded read error response")
            .expect("response");
        assert_eq!(
            response.error.expect("error").data.expect("data")["stage"],
            "terminal_pending"
        );
        assert!(harness.floors.get(&harness.identity).is_some());
        assert!(
            !owner.is_finished(),
            "read failure cannot end admitted operation custody"
        );
        observer.failing.store(false, Ordering::SeqCst);
        release_compaction_and_join(&harness).await;
        let response = owner.await.expect("owner task").expect("owned result");
        assert_eq!(
            response.error.expect("deadline error").data.expect("data")["stage"],
            "profile_restored"
        );
        assert!(harness.floors.get(&harness.identity).is_none());
        harness.run_exact_probe("after observer outage").await;
        harness.teardown().await;
    }

    /// Honest timeout semantics: when the maintenance-turn wait gives up, the
    /// error must state what actually happened to the turn - the rollback
    /// rebuild's retire quiesces it, or, when the rollback did not land, it
    /// may still be running on the floored build - instead of implying the
    /// timeout stopped anything. The member must also come back usable: the
    /// rollback rebuild restores the original profile threshold.
    ///
    /// Only the invariants that hold on BOTH branches are asserted. Which
    /// branch runs depends on wall-clock progress the test does not control,
    /// and the optional compaction-evidence clause is dropped whenever its
    /// own bounded read cannot answer.
    #[tokio::test(flavor = "multi_thread")]
    async fn compact_member_timeout_names_the_turn_fate_and_restores_the_member() {
        let harness = operator_verb_harness("worker:main", "operator-compact-timeout").await;
        let fat = "seeded transcript ballast ".repeat(160);
        for turn in 0..4 {
            harness.run_turn(format!("turn {turn}: {fat}")).await;
        }

        // A 1ms wait cannot outlast a real bridge respawn + turn: the wait
        // times out while the verb's machinery is still working.
        let response = rpc(
            &harness,
            "meta/compact_member",
            serde_json::json!({
                "identity": harness.member_alias,
                "floor_tokens": 256,
                "timeout_ms": 1,
            }),
        )
        .await;
        let message = response["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("timeout must surface a typed error: {response:#?}"));
        assert!(
            message.contains("did not complete within 1ms"),
            "the error must name the exhausted deadline: {message}"
        );
        assert!(
            message.contains("quiesced by the rollback rebuild")
                || message.contains("may still be running on the floored build")
                || message
                    .contains("exact maintenance input completed after the observation deadline"),
            "the error must state the in-flight turn's actual fate: {message}"
        );
        assert!(
            !message.contains("did not complete: "),
            "the pre-fix message shape (bare wait error, no turn fate) must be gone: {message}"
        );
        assert!(
            harness.floors.get(&harness.identity).is_none(),
            "the floor registry must be disarmed after a timed-out verb"
        );

        // The member must be usable after the rollback: a probe turn appends.
        //
        // Read off the clock the assertion actually reads. `run_turn` returns on
        // the identity's COMPLETION CURSOR; this reads the DURABLE session
        // surface, which lags it. Sampling once at cursor timing cannot tell a
        // write that has not landed from a member that never accepted the turn -
        // both render as no growth, which is how this failed CI at `3 -> 3`.
        // Third instance of this split in this file: the seeded-length read and
        // the sibling forced-compaction probe were both fixed the same way.
        let restored = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("post-timeout identity status");
        assert_eq!(
            restored.state,
            crate::identity_first::IdentityLifecycleState::Active,
            "rollback must leave the member usable: {response:#?}; status={restored:#?}"
        );
        let count_before_probe = harness.settled_transcript_count().await;
        harness
            .run_turn("post-timeout probe turn".to_string())
            .await;
        crate::test_wait::poll_until(
            &format!(
                "the rolled-back member accepted a turn and appended to the durable transcript \
                 (still {count_before_probe} messages)"
            ),
            crate::test_wait::STRUCTURAL_BACKSTOP,
            async || harness.transcript_facts().await.0 > count_before_probe,
        )
        .await;

        harness.teardown().await;
    }

    /// `meta/bound_member_transcript` on an idle member session whose tool
    /// pair straddles the naive cut point: the commit must succeed with the
    /// pair kept whole, and the resulting transcript must start with the
    /// operator marker followed by the intact pair.
    #[tokio::test(flavor = "multi_thread")]
    async fn bound_member_transcript_commits_a_pair_safe_cut() {
        let harness = operator_verb_harness("worker:main", "operator-bound-verb").await;
        let service: Arc<dyn crate::memory::hygienist::TranscriptEditSessionService> =
            Arc::clone(&harness.concrete) as _;

        // Commit one ordinary turn so the session is materialized/idle, then
        // seed the tool-pair fixture through the SAME audited edit surface
        // the verb uses (append at the end; the whole-transcript shape
        // validation admits the adjacent pair). The turn just finished, so a
        // still-draining runtime admission can answer Busy briefly; that is
        // the documented posture, retried here rather than raced.
        const SEED_TURN: &str = "seed one committed turn";
        harness.run_turn(SEED_TURN.to_string()).await;
        let session_id = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("identity status")
            .session_id
            .expect("identity session");
        // The seed turn's rows must be durable before any index is derived
        // from the transcript; completion alone does not promise that.
        wait_for_durable_turn(&service, &session_id, SEED_TURN).await;
        // Every index below is derived from this length, so it must be read
        // off a SETTLED transcript. `run_turn` waits on the identity's
        // completion cursor, which advances when the turn completes - not when
        // the turn's rows are durable in the session store, which is the
        // surface both this rewrite and the verb read. Snapshotting on the
        // cursor's timing is a racing read: under full-suite contention two
        // further rows landed between the snapshot and the rewrite and every
        // derived index was wrong (`removed` 7 against an expected 4, 1-of-5
        // full-suite runs). Quiesce on the durable surface instead.
        let seeded_len = settled_transcript_len(&service, &session_id).await;
        let (assistant, results) = tool_use_pair("call-straddle");
        let fixture = vec![
            mixed_completion_entry("job-before-cut"),
            user("older"),
            assistant,
            results,
            user("tail"),
        ];
        let fixture_len = fixture.len();
        // The turn just finished, so a still-draining runtime admission can
        // answer Busy briefly; that is the documented posture, retried here
        // rather than raced. Kept tight deliberately: the refusal is TRUE when
        // it happens, so a longer wait would only delay a correct answer and
        // hide the mechanism behind it.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let request = meerkat_core::service::SessionTranscriptRewriteRequest {
                selection: meerkat_core::TranscriptRewriteSelection::MessageRange {
                    start: seeded_len,
                    end: seeded_len,
                },
                replacement: fixture.clone(),
                reason: meerkat_core::TranscriptRewriteReason::new("test_seed"),
                actor: Some("operator-verb-test".to_string()),
                expected_parent_revision: None,
                running_behavior: meerkat_core::TranscriptEditRunningBehavior::default(),
            };
            match service
                .rewrite_session_transcript(&session_id, request)
                .await
            {
                Ok(_) => break,
                Err(SessionError::Busy { .. }) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(err) => panic!("fixture seed rewrite failed: {err}"),
            }
        }

        // Transcript is now [..seeded_len ordinary rows, mixed completion notice,
        // older, assistant, results, tail] with the results row at index
        // seeded_len + 3. keep_last = 2 naively cuts at len - 2 = seeded_len
        // + 3, which IS the tool_results row; the pair-safe cut walks back
        // one to keep the pair whole (removed = seeded_len + 2, kept = 3),
        // dropping the completion entry with the rest.
        //
        // Pin that premise before spending it on `removed` below. If anything
        // else reached the transcript, this fails naming the race rather than
        // surfacing later as unexplained index arithmetic.
        let staged_len = transcript_len(&service, &session_id).await;
        assert_eq!(
            staged_len,
            seeded_len + fixture_len,
            "the seeded fixture must be the whole of the transcript growth: expected the \
             {seeded_len} settled rows plus the {fixture_len} fixture rows, saw {staged_len}. A \
             different count means rows landed alongside the fixture and every index below is \
             derived from a stale premise."
        );
        let response = rpc(
            &harness,
            "meta/bound_member_transcript",
            serde_json::json!({
                "identity": harness.member_alias,
                "keep_last": 2,
            }),
        )
        .await;
        assert!(
            response["error"].is_null(),
            "bound_member_transcript must succeed on an idle session: {response:#?}"
        );
        let result = &response["result"];
        assert_eq!(result["bounded"], Value::Bool(true), "{result:#?}");
        assert_eq!(
            result["removed"],
            serde_json::json!(seeded_len + 2),
            "the cut must walk back off the tool_results row: {result:#?}"
        );
        assert_eq!(
            result["dropped_completion_entries"],
            serde_json::json!(1),
            "the mixed completion notice must count once when dropped: {result:#?}"
        );
        assert!(result["revision"].as_str().is_some(), "{result:#?}");

        let page = service
            .read_history(
                &session_id,
                meerkat_core::service::SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("read bounded transcript");
        assert_eq!(page.messages.len(), 4, "marker + intact pair + tail");
        assert!(
            matches!(page.messages[0], Message::SystemNotice(_)),
            "bounded transcript must lead with the operator marker"
        );
        assert!(
            matches!(page.messages[1], Message::BlockAssistant(_))
                && matches!(page.messages[2], Message::ToolResults { .. }),
            "the straddled tool pair must survive whole"
        );

        harness.teardown().await;
    }

    fn instruction_activation(
        revision_id: &str,
        activation_id: &str,
        expectation: Value,
        supersedes: Option<&str>,
        body: &str,
    ) -> Value {
        let mut activation = serde_json::json!({
            "revision": {
                "namespace": "host",
                "key": "persona",
                "revision_id": revision_id,
                "content_sha256": meerkat_core::InstructionContentDigest::for_body(body).as_str(),
            },
            "activation_id": activation_id,
            "expectation": expectation,
            "body": body,
        });
        if let Some(supersedes) = supersedes {
            activation["supersedes"] = serde_json::json!(supersedes);
        }
        activation
    }

    async fn whole_history(
        service: &Arc<dyn crate::memory::hygienist::TranscriptEditSessionService>,
        session_id: &meerkat_core::types::SessionId,
    ) -> Vec<Value> {
        service
            .read_history(
                session_id,
                meerkat_core::service::SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("read history")
            .messages
            .iter()
            .map(|message| serde_json::to_value(message).expect("message json"))
            .collect()
    }

    /// The two operator verbs that check an activation or a wired transcript
    /// service before they resolve their target refuse a generated id that
    /// is not the member's registered binding with the typed stale-binding
    /// refusal, through the operator response path, as the stdio identity
    /// methods do.
    #[tokio::test(flavor = "multi_thread")]
    async fn operator_verbs_with_preconditions_refuse_a_stale_runtime_id() {
        let harness = operator_verb_harness("worker:main", "operator-stale-runtime-id").await;
        let registered = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("identity status");
        assert_eq!(
            registered
                .agent_runtime_id
                .as_ref()
                .map(crate::identity_first::AgentRuntimeId::as_str),
            Some("rt:worker:main:0"),
            "the harness binds generation 0"
        );
        let expected = serde_json::json!({
            "code": -32000,
            "message": "identity runtime binding for worker:main points at \
                        rt:worker:main:0, but requested live member is rt:worker:main:1",
            "data": {
                "kind": "stale_identity_runtime_binding",
                "identity": "worker:main",
                "registered_runtime_member_id": "rt:worker:main:0",
                "live_runtime_member_id": "rt:worker:main:1",
                "registered_session_id": registered
                    .session_id
                    .as_ref()
                    .map(ToString::to_string),
                "live_session_id": null,
            },
        });
        for (method, params) in [
            (
                "meta/member_activate_instruction",
                serde_json::json!({
                    "identity": "rt:worker:main:1",
                    "activation": instruction_activation(
                        "r1",
                        "a1",
                        serde_json::json!({"kind": "absent"}),
                        None,
                        "Use the tools you were granted.",
                    ),
                }),
            ),
            (
                "meta/bound_member_transcript",
                serde_json::json!({"identity": "rt:worker:main:1"}),
            ),
        ] {
            let response = rpc(&harness, method, params).await;
            assert_eq!(response["error"], expected, "{method}: {response:#?}");
        }
        harness.teardown().await;
    }

    /// `meta/member_activate_instruction` appends forward: the transcript
    /// before the activation is unchanged and one activation row follows it.
    /// Re-applying the effective activation is a typed `duplicate` with no
    /// second row, and the read verb returns the one durable record.
    #[tokio::test(flavor = "multi_thread")]
    async fn member_activate_instruction_appends_forward_and_reapply_is_duplicate() {
        let harness = operator_verb_harness("worker:main", "operator-instruction-verb").await;
        let service: Arc<dyn crate::memory::hygienist::TranscriptEditSessionService> =
            Arc::clone(&harness.concrete) as _;
        const SEED_TURN: &str = "seed one committed turn";
        harness.run_turn(SEED_TURN.to_string()).await;
        let session_id = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("identity status")
            .session_id
            .expect("identity session");
        wait_for_durable_turn(&service, &session_id, SEED_TURN).await;
        let before = whole_history(&service, &session_id).await;

        const BODY: &str = "Use the tools you were granted; your skills changed.";
        let activation = instruction_activation(
            "r1",
            "a1",
            serde_json::json!({"kind": "absent"}),
            None,
            BODY,
        );
        let params = serde_json::json!({
            "identity": harness.member_alias,
            "activation": activation,
        });
        let response = rpc(&harness, "meta/member_activate_instruction", params.clone()).await;
        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(
            response["result"]["receipt"]["disposition"],
            serde_json::json!("applied"),
            "{response:#?}"
        );

        let after = whole_history(&service, &session_id).await;
        assert_eq!(after.len(), before.len() + 1, "exactly one row is appended");
        assert_eq!(
            after[..before.len()],
            before[..],
            "the transcript before the activation is not rewritten"
        );
        assert!(
            after[before.len()].to_string().contains(BODY),
            "the appended row carries the instruction: {}",
            after[before.len()]
        );

        let again = rpc(&harness, "meta/member_activate_instruction", params).await;
        assert!(again["error"].is_null(), "{again:#?}");
        assert_eq!(
            again["result"]["receipt"]["disposition"],
            serde_json::json!("duplicate"),
            "{again:#?}"
        );
        assert_eq!(
            whole_history(&service, &session_id).await.len(),
            after.len(),
            "a duplicate appends nothing"
        );

        // The owner's own refusals stay typed and append nothing: a body that
        // does not match its declared digest, and a stale expectation (the
        // host believed no activation was effective).
        let mut mismatched = instruction_activation(
            "r2",
            "a2",
            serde_json::json!({"kind": "effective", "activation_id": "a1"}),
            Some("a1"),
            "the second body",
        );
        mismatched["body"] = serde_json::json!("a different body");
        let stale = instruction_activation(
            "r2",
            "a2",
            serde_json::json!({"kind": "absent"}),
            None,
            "the second body",
        );
        for (activation, refusal) in [
            (mismatched, "digest_mismatch"),
            (stale, "effective_activation_conflict"),
        ] {
            let refused = rpc(
                &harness,
                "meta/member_activate_instruction",
                serde_json::json!({"identity": harness.member_alias, "activation": activation}),
            )
            .await;
            assert_eq!(
                refused["error"]["code"],
                serde_json::json!(-32602),
                "{refused:#?}"
            );
            assert_eq!(
                refused["error"]["data"]["class"],
                serde_json::json!("session"),
                "{refused:#?}"
            );
            assert_eq!(
                refused["error"]["data"]["instruction_activation_error_code"],
                serde_json::json!(refusal),
                "{refused:#?}"
            );
        }
        assert_eq!(
            whole_history(&service, &session_id).await.len(),
            after.len(),
            "a refused activation appends nothing"
        );

        let page = rpc(
            &harness,
            "meta/member_instruction_activations",
            serde_json::json!({
                "identity": harness.member_alias,
                "namespace": "host",
                "key": "persona",
            }),
        )
        .await;
        assert!(page["error"].is_null(), "{page:#?}");
        let records = page["result"]["page"]["records"]
            .as_array()
            .expect("records array");
        assert_eq!(records.len(), 1, "{page:#?}");
        assert_eq!(
            records[0]["identity"]["activation_id"],
            serde_json::json!("a1"),
            "{page:#?}"
        );

        harness.teardown().await;
    }

    /// Seed one committed turn, activate one keyed host instruction on the
    /// member through `meta/member_activate_instruction`, and return the
    /// member's roster identity, the source a mob fork names.
    async fn seed_activated_source(
        harness: &OperatorVerbHarness,
        member: &str,
    ) -> meerkat_mob::ids::AgentIdentity {
        let service: Arc<dyn crate::memory::hygienist::TranscriptEditSessionService> =
            Arc::clone(&harness.concrete) as _;
        const SEED_TURN: &str = "seed one committed turn";
        harness.run_turn(SEED_TURN.to_string()).await;
        let session_id = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("identity status")
            .session_id
            .expect("identity session");
        wait_for_durable_turn(&service, &session_id, SEED_TURN).await;
        let activation = instruction_activation(
            "r1",
            "a1",
            serde_json::json!({"kind": "absent"}),
            None,
            "Answer in French.",
        );
        let response = rpc(
            harness,
            "meta/member_activate_instruction",
            serde_json::json!({ "identity": harness.member_alias, "activation": activation }),
        )
        .await;
        assert!(response["error"].is_null(), "{response:#?}");
        meerkat_mob::ids::AgentIdentity::from(
            crate::member_comms_id::mob_member_id_str(member).into_owned(),
        )
    }

    /// The call `fork_off` makes: a fork that starts its own first turn. Like
    /// Meerkat's `fork_off`, it forks a turn-driven child whatever the role
    /// profile's runtime mode is, since the bounded first turn tracks its
    /// completion.
    async fn self_starting_fork(
        harness: &OperatorVerbHarness,
        source: &meerkat_mob::ids::AgentIdentity,
    ) -> Result<meerkat_mob::ForkMemberBoundedRunOutcome, meerkat_mob::BoundedMemberRunError> {
        harness
            .runtime
            .mob_handle()
            .fork_member_then_run_bounded(
                source,
                meerkat_mob::SpawnMemberSpec::new(
                    meerkat_mob::ProfileName::from("worker"),
                    meerkat_mob::ids::AgentIdentity::from("fork-a"),
                )
                .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven)
                .with_initial_message("Summarize the conversation so far."),
                None,
                "summary",
                4096,
                meerkat_core::DurableForkSourceAdmission::Quiescent,
            )
            .await
    }

    fn assert_policy_refusal(
        outcome: Result<
            meerkat_mob::ForkMemberBoundedRunOutcome,
            meerkat_mob::BoundedMemberRunError,
        >,
    ) {
        match outcome {
            Err(meerkat_mob::BoundedMemberRunError::Admission(
                meerkat_mob::MobError::ForkInstructionRefused { refusal, .. },
            )) => assert_eq!(
                refusal.cause,
                meerkat_core::ForkInstructionRefusalCause::PolicyRefuses,
                "{refusal:?}"
            ),
            other => panic!("expected the typed fork instruction refusal, got {other:?}"),
        }
    }

    async fn assert_no_fork_member(harness: &OperatorVerbHarness) {
        assert!(
            harness
                .runtime
                .mob_handle()
                .list_members()
                .await
                .iter()
                .all(|entry| entry.agent_identity.as_str() != "fork-a"),
            "a refused fork commits nothing"
        );
    }

    /// Meerkat's default: a self-starting fork of a member with an activated
    /// host instruction is refused, typed, before anything is committed.
    #[tokio::test(flavor = "multi_thread")]
    async fn fork_off_of_an_activated_member_is_refused_typed_by_default() {
        let harness = operator_verb_harness_with_fork_policy(
            "worker:main",
            "fork-policy-default",
            meerkat_core::ForkInstructionPolicy::RequireExplicitActivation,
        )
        .await;
        let source = seed_activated_source(&harness, "worker:main").await;
        assert_policy_refusal(self_starting_fork(&harness, &source).await);
        assert_no_fork_member(&harness).await;
        harness.teardown().await;
    }

    /// The host opted in: the fork re-activates the inherited key in the
    /// child, and the child's first turn runs.
    #[tokio::test(flavor = "multi_thread")]
    async fn fork_off_of_an_activated_member_runs_when_the_host_opts_in() {
        let harness = operator_verb_harness_with_fork_policy(
            "worker:main",
            "fork-policy-reactivate",
            meerkat_core::ForkInstructionPolicy::ReactivateInherited,
        )
        .await;
        let source = seed_activated_source(&harness, "worker:main").await;
        let outcome = self_starting_fork(&harness, &source).await;
        assert!(outcome.is_ok(), "the re-activated child runs: {outcome:?}");
        harness.teardown().await;
    }

    /// `refuse`: every fork that would inherit an activation is refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn fork_off_of_an_activated_member_is_refused_under_refuse() {
        let harness = operator_verb_harness_with_fork_policy(
            "worker:main",
            "fork-policy-refuse",
            meerkat_core::ForkInstructionPolicy::Refuse,
        )
        .await;
        let source = seed_activated_source(&harness, "worker:main").await;
        assert_policy_refusal(self_starting_fork(&harness, &source).await);
        assert_no_fork_member(&harness).await;
        harness.teardown().await;
    }

    /// Meerkat's default leaves a host-driven fork to the host: the console
    /// fork commits the child, and the host activates it before its first
    /// turn (the child's first turn alone is refused until it does).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_console_fork_of_an_activated_member_commits_under_the_default() {
        let harness = operator_verb_harness_with_fork_policy(
            "worker:main",
            "fork-policy-console",
            meerkat_core::ForkInstructionPolicy::RequireExplicitActivation,
        )
        .await;
        seed_activated_source(&harness, "worker:main").await;
        let fork = harness
            .runtime
            .fork_member(
                &harness.member_alias,
                meerkat_mob::SpawnMemberSpec::new(
                    meerkat_mob::ProfileName::from("worker"),
                    meerkat_mob::ids::AgentIdentity::from("fork-b"),
                ),
                None,
            )
            .await;
        assert!(
            fork.is_ok(),
            "a host-driven fork commits under the default: {fork:?}"
        );
        harness.teardown().await;
    }

    /// Mid-turn the activation never interleaves with the running turn:
    /// Meerkat's member door waits at the turn-finalization boundary and
    /// applies there, so the activation lands after the held turn's reply.
    /// Nothing in Meta waits, polls or retries; a refusal at that boundary
    /// (work still queued) is the typed `session_busy` admission class
    /// asserted in the mapping test below.
    #[tokio::test(flavor = "multi_thread")]
    async fn member_activate_instruction_mid_turn_applies_at_the_turn_boundary() {
        let harness =
            Arc::new(operator_verb_harness("worker:main", "operator-instruction-busy").await);
        let service: Arc<dyn crate::memory::hygienist::TranscriptEditSessionService> =
            Arc::clone(&harness.concrete) as _;
        harness
            .run_turn("seed one committed turn".to_string())
            .await;
        harness.gate_armed.store(true, Ordering::SeqCst);
        const HELD_TURN: &str = "held turn";
        let admission = harness
            .identity_runtime
            .send_admission_tracked(
                &harness.identity,
                None,
                &meerkat_core::ContentInput::Text(HELD_TURN.to_string()),
                meerkat_core::types::HandlingMode::Queue,
                None,
            )
            .await
            .expect("held turn admitted");
        // Typed: the gated LLM client fires `entered` once the turn is parked
        // inside the model call. The timeout is only a hang guard.
        tokio::time::timeout(Duration::from_mins(1), harness.entered.notified())
            .await
            .expect("the held turn never reached the LLM call");

        let caller = Arc::clone(&harness);
        let activation = tokio::spawn(async move {
            rpc(
                &caller,
                "meta/member_activate_instruction",
                serde_json::json!({
                    "identity": caller.member_alias,
                    "activation": instruction_activation(
                        "r1",
                        "a1",
                        serde_json::json!({"kind": "absent"}),
                        None,
                        "issued while a turn runs",
                    ),
                }),
            )
            .await
        });
        harness.gate_armed.store(false, Ordering::SeqCst);
        harness.release.notify_waiters();
        let response = tokio::time::timeout(Duration::from_mins(1), activation)
            .await
            .expect("the activation never returned")
            .expect("activation task");
        harness
            .identity_runtime
            .wait_for_completion(
                &harness.identity,
                admission.completion_baseline,
                Duration::from_mins(1),
            )
            .await
            .expect("held turn completes after release");
        assert!(response["error"].is_null(), "{response:#?}");
        assert_eq!(
            response["result"]["receipt"]["disposition"],
            serde_json::json!("applied"),
            "{response:#?}"
        );
        let session_id = harness
            .identity_runtime
            .status(&harness.identity)
            .await
            .expect("identity status")
            .session_id
            .expect("identity session");
        let history = whole_history(&service, &session_id).await;
        let held_input = history
            .iter()
            .position(|message| message.to_string().contains(HELD_TURN))
            .expect("the held turn's input is in the transcript");
        let activation_index =
            response["result"]["receipt"]["record"]["projection_witness"]["message_index"]
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .expect("projection witness message index");
        assert!(
            activation_index > held_input + 1,
            "the activation must land after the held turn's reply, not inside it: \
             activation at {activation_index}, held input at {held_input}: {history:#?}"
        );
        harness.teardown().await;
    }

    /// Every activation failure class keeps its own typed shape: admission
    /// codes, session refusals (with the owner's typed activation code),
    /// runtime authority and owner-task failures never collapse into one.
    #[test]
    fn member_instruction_errors_map_per_class() {
        use meerkat_core::InstructionActivationAdmissionErrorCode as Code;
        use meerkat_mob::MemberInstructionActivationError as Error;
        let map = |error: Error| {
            let response = member_instruction_error_response(serde_json::json!(1), error);
            let error = response.error.expect("an error response");
            (error.code, error.data.expect("typed data"))
        };
        for (code, rpc_code, wire) in [
            (
                Code::TargetNotMaterialized,
                MEMBER_INSTRUCTION_REFUSED_CODE,
                "target_not_materialized",
            ),
            (
                Code::SessionBusy,
                MEMBER_INSTRUCTION_REFUSED_CODE,
                "session_busy",
            ),
            (
                Code::LiveChannelOpen,
                MEMBER_INSTRUCTION_REFUSED_CODE,
                "live_channel_open",
            ),
            (
                Code::ExternalWriteFenceConflict,
                MEMBER_INSTRUCTION_REFUSED_CODE,
                "external_write_fence_conflict",
            ),
            (
                Code::ExternalWriteFenceBackoff,
                MEMBER_INSTRUCTION_REFUSED_CODE,
                "external_write_fence_backoff",
            ),
            (
                Code::UnsupportedCurrentLowering,
                -32602,
                "unsupported_current_lowering",
            ),
            (
                Code::DurabilityUnavailable,
                -32603,
                "durability_unavailable",
            ),
        ] {
            assert_eq!(
                map(Error::Admission {
                    code,
                    message: "refused".to_string(),
                }),
                (
                    rpc_code,
                    serde_json::json!({"class": "admission", "instruction_activation_code": wire}),
                ),
                "{code:?}"
            );
        }
        let session_id = meerkat_core::types::SessionId::new();
        assert_eq!(
            map(Error::Session(SessionError::Busy { id: session_id })),
            (
                MEMBER_INSTRUCTION_REFUSED_CODE,
                serde_json::json!({"class": "session", "session_error_code": "SESSION_BUSY"}),
            )
        );
        let refused = |code: meerkat_core::InstructionActivationErrorCode| {
            Error::Session(SessionError::FailedWithData {
                message: "refused".to_string(),
                data: serde_json::json!({"instruction_activation_code": code}),
            })
        };
        let (rpc_code, data) = map(refused(
            meerkat_core::InstructionActivationErrorCode::DigestMismatch,
        ));
        assert_eq!(rpc_code, -32602);
        assert_eq!(data["class"], serde_json::json!("session"));
        assert_eq!(
            data["instruction_activation_error_code"],
            serde_json::json!("digest_mismatch")
        );
        let (rpc_code, data) = map(refused(
            meerkat_core::InstructionActivationErrorCode::MalformedActivationHistory,
        ));
        assert_eq!(rpc_code, -32603);
        assert_eq!(
            data["instruction_activation_error_code"],
            serde_json::json!("malformed_activation_history")
        );
        assert_eq!(
            map(Error::Runtime(
                meerkat_runtime::RuntimeDriverError::Destroyed
            )),
            (-32603, serde_json::json!({"class": "runtime"}))
        );
        assert_eq!(
            map(Error::OwnerTask("owner task panicked".to_string())),
            (-32603, serde_json::json!({"class": "owner_task"}))
        );
    }

    /// An identity with no member session is refused typed: Meerkat's
    /// `target_not_materialized` admission class, with nothing appended.
    #[tokio::test(flavor = "multi_thread")]
    async fn member_activate_instruction_refuses_unknown_identities_typed() {
        let harness = operator_verb_harness("worker:main", "operator-instruction-unknown").await;
        let response = rpc(
            &harness,
            "meta/member_activate_instruction",
            serde_json::json!({
                "identity": "worker:nobody",
                "activation": instruction_activation(
                    "r1",
                    "a1",
                    serde_json::json!({"kind": "absent"}),
                    None,
                    "body",
                ),
            }),
        )
        .await;
        assert_eq!(
            response["error"]["code"],
            serde_json::json!(MEMBER_INSTRUCTION_REFUSED_CODE),
            "{response:#?}"
        );
        assert_eq!(
            response["error"]["data"],
            serde_json::json!({
                "class": "admission",
                "instruction_activation_code": "target_not_materialized",
            }),
            "{response:#?}"
        );
        harness.teardown().await;
    }

    /// `meta/bound_member_transcript` while the member is mid-turn: the
    /// service's `SessionError::Busy` surfaces as the verb's typed refusal.
    #[tokio::test(flavor = "multi_thread")]
    async fn bound_member_transcript_refuses_running_sessions_typed() {
        let harness = operator_verb_harness("worker:main", "operator-bound-busy").await;

        // Commit one ordinary turn first so the transcript has messages past
        // keep_last = 1 (a zero-length transcript would no-op before ever
        // reaching the rewrite's Busy check).
        harness
            .run_turn("seed one committed turn".to_string())
            .await;

        // Arm the gate, then hold one member turn in flight inside the LLM
        // call so the session's active runtime admission is observably held.
        harness.gate_armed.store(true, Ordering::SeqCst);
        let admission = harness
            .identity_runtime
            .send_admission_tracked(
                &harness.identity,
                None,
                &meerkat_core::ContentInput::Text("held turn".to_string()),
                meerkat_core::types::HandlingMode::Queue,
                None,
            )
            .await
            .expect("held turn admitted");
        // Backstop only: the admitted turn either reaches the gated LLM call or
        // it never will. Generous so full-suite CPU starvation cannot reach it.
        let deadline = std::time::Instant::now() + Duration::from_mins(1);
        while !harness.in_call.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "the held turn never reached the LLM call: the gate was armed and the turn was \
                 admitted, but `in_call` was never set"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let response = rpc(
            &harness,
            "meta/bound_member_transcript",
            serde_json::json!({
                "identity": harness.member_alias,
                "keep_last": 1,
            }),
        )
        .await;
        assert_eq!(
            response["error"]["code"],
            serde_json::json!(OPERATOR_SESSION_BUSY_CODE),
            "a running session must surface the typed Busy refusal: {response:#?}"
        );
        assert!(
            response["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("quiesce")),
            "the refusal must document quiesce-first: {response:#?}"
        );

        // Release the held turn so shutdown is clean.
        harness.gate_armed.store(false, Ordering::SeqCst);
        harness.release.notify_waiters();
        harness
            .identity_runtime
            .wait_for_completion(
                &harness.identity,
                admission.completion_baseline,
                Duration::from_secs(30),
            )
            .await
            .expect("held turn completed after release");

        harness.teardown().await;
    }

    #[test]
    fn optional_u64_param_rejects_zero_and_non_integers() {
        let params = serde_json::json!({ "floor_tokens": 0 });
        assert!(optional_u64_param(&params, "floor_tokens").is_err());
        let params = serde_json::json!({ "floor_tokens": "many" });
        assert!(optional_u64_param(&params, "floor_tokens").is_err());
        let params = serde_json::json!({});
        assert_eq!(optional_u64_param(&params, "floor_tokens").unwrap(), None);
        let params = serde_json::json!({ "floor_tokens": 2048 });
        assert_eq!(
            optional_u64_param(&params, "floor_tokens").unwrap(),
            Some(2048)
        );
    }
}
