//! `turn/*` method handlers.

use std::sync::Arc;

use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use meerkat_contracts::SkillsParams;
use meerkat_core::ContentInput;
use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::PublicTurnToolOverlay;
use meerkat_core::skills::{SkillKey, SkillRef};

use super::skills::reject_retired_skill_references;
use super::{RpcResponseExt, parse_params, parse_session_id_for_runtime};
use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;
use meerkat::surface::RequestContext;
use meerkat_runtime::SessionServiceRuntimeExt;

// ---------------------------------------------------------------------------
// Param types
// ---------------------------------------------------------------------------

/// Parameters for `turn/start`.
///
/// `provider_params` and `auth_binding` carry the canonical Inherit/Set/Clear
/// tri-state via [`TurnMetadataOverride`]: `None` keeps the session's current
/// value, `Some(Set)` overrides it, `Some(Clear)` removes it. Unknown fields
/// (including the retired `clear_*` split wire form) fail closed at the serde
/// boundary. This prevents cross-realm credential bleed in multi-tenant
/// setups (deferral §2 / Dogma §10).
///
/// `max_tokens`, `output_schema` and `structured_output_retries` are build
/// settings, fixed when the session's agent is built: a runtime-routed turn
/// has no carrier for them. They are refused at the serde boundary, before
/// any session work; an explicit `null` keeps meaning "inherit". The fields
/// stay on the type for typed callers, which
/// [`start_turn_with_params`] refuses the same way.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartTurnParams {
    pub session_id: String,
    pub prompt: ContentInput,
    #[serde(default)]
    pub skill_refs: Option<Vec<SkillRef>>,
    #[serde(default, deserialize_with = "reject_retired_skill_references")]
    pub skill_references: Option<Vec<String>>,
    #[serde(default)]
    pub turn_tool_overlay: Option<PublicTurnToolOverlay>,
    #[serde(default)]
    pub additional_instructions: Option<Vec<String>>,
    #[serde(default)]
    pub keep_alive: Option<bool>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    /// Exact configured local-server route for a self-hosted model.
    #[serde(default)]
    pub self_hosted_server_id: Option<String>,
    /// Build setting: refused on a turn (see the type docs).
    #[serde(default, deserialize_with = "refuse_max_tokens_on_turn")]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Build setting: refused on a turn (see the type docs).
    #[serde(default, deserialize_with = "refuse_output_schema_on_turn")]
    pub output_schema: Option<serde_json::Value>,
    /// Build setting: refused on a turn (see the type docs).
    #[serde(default, deserialize_with = "refuse_structured_output_retries_on_turn")]
    pub structured_output_retries: Option<u32>,
    #[serde(default)]
    pub provider_params: Option<
        TurnMetadataOverride<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    >,
    #[serde(default)]
    pub auth_binding: Option<TurnMetadataOverride<meerkat_core::AuthBindingRef>>,
    /// Host-attached injected context for this turn. Each entry materializes
    /// as a separate typed injected-context user-channel message immediately
    /// before the turn's user message, in order. Injected context is
    /// excluded from semantic-memory indexing.
    #[serde(default)]
    pub injected_context: Option<Vec<ContentInput>>,
    /// Request-only host context for this runtime-owned logical turn.
    #[serde(default)]
    pub transient_turn_context: Option<meerkat_core::lifecycle::run_primitive::TurnRequestContext>,
    /// `false`: this turn's events are not written as `session/event`
    /// notifications. A caller that reads them from a `session/stream_open`
    /// subscription, or not at all, sends it so each event reaches it once.
    /// Absent or `true` keeps today's behaviour. If another input shares the
    /// turn and its request wants the events, they are written. Since 0.9.5;
    /// an older server refuses the field.
    #[serde(default)]
    pub session_events: Option<bool>,
}

/// Where a session's build settings are set, for a refusal on a turn.
pub(crate) const SET_BUILD_SETTINGS_ON_CREATE: &str =
    "set it on session/create, a deferred create included";

/// The refusal of build setting `setting` on a turn, naming where it is set.
pub(crate) fn build_setting_on_turn_message(setting: &str, remedy: &str) -> String {
    format!(
        "{setting} is a build setting fixed when the session's agent is built; \
         a turn cannot override it ({remedy})"
    )
}

/// The first build setting a turn names, in field order.
pub(crate) fn first_build_setting_on_turn(
    max_tokens: bool,
    output_schema: bool,
    structured_output_retries: bool,
) -> Option<&'static str> {
    [
        max_tokens.then_some("max_tokens"),
        output_schema.then_some("output_schema"),
        structured_output_retries.then_some("structured_output_retries"),
    ]
    .into_iter()
    .flatten()
    .next()
}

/// Refuse a present build setting while the params are parsed. `null` is
/// "inherit", as it always was.
fn refuse_build_setting_on_turn<'de, D, T>(
    deserializer: D,
    setting: &'static str,
) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<serde::de::IgnoredAny>::deserialize(deserializer)? {
        None => Ok(None),
        Some(_) => Err(serde::de::Error::custom(build_setting_on_turn_message(
            setting,
            SET_BUILD_SETTINGS_ON_CREATE,
        ))),
    }
}

fn refuse_max_tokens_on_turn<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    refuse_build_setting_on_turn(deserializer, "max_tokens")
}

fn refuse_output_schema_on_turn<'de, D>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    refuse_build_setting_on_turn(deserializer, "output_schema")
}

fn refuse_structured_output_retries_on_turn<'de, D>(
    deserializer: D,
) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    refuse_build_setting_on_turn(deserializer, "structured_output_retries")
}

/// Parameters for `turn/interrupt` — canonical wire type from contracts.
pub use meerkat_contracts::wire::InterruptParams;

/// Result for `turn/interrupt` — canonical wire type from contracts.
///
/// Carries the generated interrupt public-result tri-state: a live run was
/// cancelled (`interrupted`), or a staged (not-yet-promoted) session interrupt
/// was a typed no-op terminal (`staged_noop`). NotFound/SessionBusy/Conflict
/// remain typed JSON-RPC errors. This mirrors the REST surface exactly — the
/// previous hand-shaped `{"interrupted": true}` claimed a live cancellation
/// for every non-error outcome.
pub use meerkat_contracts::wire::{InterruptResult, WireInterruptOutcome};

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Result for `turn/start` — uses canonical wire type from contracts.
pub type TurnResult = meerkat_contracts::WireRunResult;

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn canonical_skill_ids(
    _runtime: &SessionRuntime,
    skill_refs: Option<Vec<SkillRef>>,
) -> Result<Option<Vec<SkillKey>>, meerkat_core::skills::SkillError> {
    let params = SkillsParams {
        preload_skills: None,
        skill_refs,
    };
    Ok(params.canonical_skill_keys())
}

/// Collect per-turn override fields into a struct for `SessionRuntime::start_turn`.
#[derive(Clone, Debug, Default)]
pub struct TurnOverrides {
    pub keep_alive: Option<bool>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub self_hosted_server_id: Option<String>,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,
    pub output_schema: Option<serde_json::Value>,
    pub structured_output_retries: Option<u32>,
    pub provider_params: Option<
        TurnMetadataOverride<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    >,
    pub auth_binding: Option<TurnMetadataOverride<meerkat_core::AuthBindingRef>>,
    pub transient_turn_context: Option<meerkat_core::lifecycle::run_primitive::TurnRequestContext>,
}

impl TurnOverrides {
    pub(crate) fn is_empty(&self) -> bool {
        self.keep_alive.is_none()
            && self.model.is_none()
            && self.provider.is_none()
            && self.self_hosted_server_id.is_none()
            && self.max_tokens.is_none()
            && self.system_prompt.is_none()
            && self.output_schema.is_none()
            && self.structured_output_retries.is_none()
            && self.provider_params.is_none()
            && self.auth_binding.is_none()
            && self.transient_turn_context.is_none()
    }
}

/// Handle `turn/start`.
pub async fn handle_start(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
    notification_sink: &NotificationSink,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    request_context: Option<RequestContext>,
) -> RpcResponse {
    let params: StartTurnParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    start_turn_with_params(
        id,
        params,
        runtime,
        notification_sink,
        runtime_adapter,
        request_context,
    )
    .await
}

/// Typed `turn/start` entrypoint shared with identity-native callers
/// (`mob/turn_start`). Takes a fully-formed [`StartTurnParams`] so callers can
/// route typed params without re-serializing through a JSON `Map`.
///
/// A build setting (`max_tokens`, `output_schema`,
/// `structured_output_retries`) is refused first, before any session work:
/// wire `turn/start` refuses it while parsing, and a typed caller here.
pub async fn start_turn_with_params(
    id: Option<RpcId>,
    params: StartTurnParams,
    runtime: Arc<SessionRuntime>,
    notification_sink: &NotificationSink,
    _runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    request_context: Option<RequestContext>,
) -> RpcResponse {
    if let Some(setting) = first_build_setting_on_turn(
        params.max_tokens.is_some(),
        params.output_schema.is_some(),
        params.structured_output_retries.is_some(),
    ) {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            build_setting_on_turn_message(setting, SET_BUILD_SETTINGS_ON_CREATE),
        );
    }

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, &runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    if let Some(context) = request_context.as_ref()
        && context.cancel_already_requested()
    {
        return RpcResponse::error(
            id,
            error::REQUEST_CANCELLED,
            "request cancelled before start",
        );
    }

    // Set up MCP lifecycle event forwarding. Agent execution events flow
    // through SessionRuntimeExecutor's own channel, not this one.
    let (mcp_event_tx, mut mcp_event_rx) =
        mpsc::channel::<EventEnvelope<AgentEvent>>(NOTIFICATION_CHANNEL_CAPACITY);
    let sink = notification_sink.clone();
    let sid_clone = session_id.clone();
    tokio::spawn(async move {
        while let Some(event) = mcp_event_rx.recv().await {
            sink.emit_event(&sid_clone, &event).await;
        }
    });

    let skill_refs = match canonical_skill_ids(&runtime, params.skill_refs) {
        Ok(r) => r,
        Err(e) => {
            return RpcResponse::error(
                id,
                crate::error::INVALID_PARAMS,
                format!("Invalid skill_refs: {e}"),
            );
        }
    };

    let overrides = TurnOverrides {
        keep_alive: params.keep_alive,
        model: params.model,
        provider: params.provider,
        self_hosted_server_id: params.self_hosted_server_id,
        max_tokens: params.max_tokens,
        system_prompt: params.system_prompt,
        output_schema: params.output_schema,
        structured_output_retries: params.structured_output_retries,
        provider_params: params.provider_params,
        auth_binding: params.auth_binding,
        transient_turn_context: params.transient_turn_context,
    };

    let result = match runtime
        .start_turn_via_runtime_with_request_context(
            &session_id,
            params.prompt,
            params.injected_context.unwrap_or_default(),
            mcp_event_tx,
            skill_refs,
            params.turn_tool_overlay.map(Into::into),
            params.additional_instructions,
            if overrides.is_empty() {
                None
            } else {
                Some(overrides)
            },
            request_context,
            params.session_events.unwrap_or(true),
        )
        .await
    {
        Ok(r) => r,
        // The typed data (the turn error, a callback-pending payload) stays
        // on the response with its typed code.
        Err(rpc_err) => {
            return RpcResponse::from_error(id, rpc_err);
        }
    };

    let mut response: TurnResult = result.into();
    response.session_ref = runtime
        .realm_id()
        .map(|realm| meerkat_contracts::format_session_ref(&realm, &response.session_id));
    RpcResponse::success(id, response)
}

/// Handle `turn/interrupt`.
pub async fn handle_interrupt(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
    #[cfg(feature = "mob")] _mob_state: &meerkat_mob_mcp::MobMcpState,
) -> RpcResponse {
    let params: InterruptParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };

    // `SessionRuntime::interrupt` surfaces the generated interrupt
    // public-result tri-state (Interrupted vs StagedNoop) for non-error
    // outcomes; NotFound/SessionBusy/Conflict arrive as typed RpcErrors.
    // The handler serializes the contracts `InterruptResult` wire type —
    // no hand-shaped payloads — so RPC reports a staged-session no-op
    // honestly instead of fabricating a live cancellation.
    match runtime.interrupt(&session_id).await {
        Ok(outcome) => RpcResponse::success(
            id,
            InterruptResult::from_outcome(session_id.to_string(), outcome),
        ),
        Err(err) => RpcResponse::from_error(id, err),
    }
}

/// Handle `turn/stop_run`: the run-fenced Stop.
///
/// Stops the exact `run_id` and returns the typed receipt. A stale run id is
/// the ordinary `not_current` outcome, never an error; a malformed one is an
/// invalid-params error.
pub async fn handle_stop_run(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let params: meerkat_contracts::StopRunParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let session_id = match parse_session_id_for_runtime(id.clone(), &params.session_id, runtime) {
        Ok(sid) => sid,
        Err(resp) => return resp,
    };
    let run_id = match meerkat::surface::parse_wire_run_id(&params.run_id) {
        Ok(run_id) => run_id,
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
    };
    let receipt = match runtime.stop_run(&session_id, &run_id, params.reason).await {
        Ok(receipt) => receipt,
        Err(err) => return RpcResponse::from_error(id, err),
    };
    match meerkat::surface::wire_run_stop_receipt(&receipt) {
        Ok(receipt) => RpcResponse::success(
            id,
            meerkat_contracts::StopRunResult {
                session_id: session_id.to_string(),
                receipt,
            },
        ),
        Err(message) => RpcResponse::error(id, error::INTERNAL_ERROR, message),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod start_turn_params_tests {
    use super::StartTurnParams;
    use meerkat_core::ContentInput;

    /// The optional `injected_context` param parses as a list of
    /// content inputs (text or block form), matching the `prompt` treatment.
    #[test]
    fn start_turn_params_accept_injected_context() {
        let params: StartTurnParams = serde_json::from_value(serde_json::json!({
            "session_id": "session-1",
            "prompt": "hello",
            "injected_context": [
                "ambient one",
                [{"type": "text", "text": "ambient two"}]
            ]
        }))
        .unwrap();
        let entries = params.injected_context.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(&entries[0], ContentInput::Text(text) if text == "ambient one"));
        assert!(matches!(&entries[1], ContentInput::Blocks(blocks) if blocks.len() == 1));
    }

    /// Absent `injected_context` stays `None` — the pre-existing wire shape
    /// is unchanged (deny_unknown_fields still accepts the old payload).
    #[test]
    fn start_turn_params_default_injected_context_to_none() {
        let params: StartTurnParams = serde_json::from_value(serde_json::json!({
            "session_id": "session-1",
            "prompt": "hello"
        }))
        .unwrap();
        assert!(params.injected_context.is_none());
    }

    /// Build settings are fixed when a session's agent is built, so
    /// `turn/start` refuses them while parsing its params, before any
    /// session work. An explicit `null` still means "inherit".
    #[test]
    fn start_turn_params_reject_build_settings_while_parsing() {
        for (field, value) in [
            ("max_tokens", serde_json::json!(1024)),
            ("output_schema", serde_json::json!({"type": "object"})),
            ("structured_output_retries", serde_json::json!(5)),
        ] {
            let mut params = serde_json::json!({
                "session_id": "session-1",
                "prompt": "hello"
            });
            params[field] = value;
            let error = serde_json::from_value::<StartTurnParams>(params)
                .map(|_| ())
                .expect_err("a build setting must be refused while parsing turn/start params");
            let message = error.to_string();
            assert!(
                message.contains(field) && message.contains("session/create"),
                "the refusal names the setting and where to set it: {message}"
            );

            let mut inherit = serde_json::json!({
                "session_id": "session-1",
                "prompt": "hello"
            });
            inherit[field] = serde_json::Value::Null;
            serde_json::from_value::<StartTurnParams>(inherit)
                .expect("an explicit null keeps inheriting the session's setting");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod interrupt_wire_tests {
    use super::{InterruptResult, WireInterruptOutcome};

    /// K17 regression: a staged-session interrupt must serialize as the
    /// honest tri-state (`interrupted: false`, `result: staged_noop`) — RPC
    /// and REST share the same generated wire contract, so the previous RPC
    /// divergence (`{"interrupted": true}` for every Ok) cannot reappear.
    #[test]
    fn staged_noop_interrupt_serializes_honest_tri_state() {
        let staged = InterruptResult::from_outcome(
            "session-1".to_string(),
            WireInterruptOutcome::StagedNoop,
        );
        let value = serde_json::to_value(&staged).unwrap();
        assert_eq!(value["session_id"], "session-1");
        assert_eq!(value["interrupted"], false);
        assert_eq!(value["result"], "staged_noop");

        let interrupted = InterruptResult::from_outcome(
            "session-1".to_string(),
            WireInterruptOutcome::Interrupted,
        );
        let value = serde_json::to_value(&interrupted).unwrap();
        assert_eq!(value["interrupted"], true);
        assert_eq!(value["result"], "interrupted");
    }
}
