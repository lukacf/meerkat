//! RPC handler implementations for mob member operations.

use base64::Engine;
use meerkat_contracts::WireRuntimeBinding;
use meerkat_core::ContentInput;
use meerkat_mob::launch::{ForkContext, MemberLaunchMode};
use meerkat_mob::runtime::reconcile::MemberFilter;
use meerkat_mob::{HelperOptions, MobBackendKind, MobRuntimeMode, ProfileName, SpawnMemberSpec};
use serde_json::Value;

use crate::blob_store::is_valid_blob_id_value;
use crate::mob_handle_runtime::{
    assert_member_accepts_images_with_config, is_recoverable_lifecycle_cleanup_error,
    member_entry_to_json, model_routing_status_for_member, model_routing_status_for_session,
    resolved_tools_for_member, resolved_tools_for_session, send_message_on_mob_with_interaction,
    topology_restore_failed_peer_ids, topology_restore_warning_json,
};
use crate::unified_runtime::UnifiedRuntime;

use super::{JSONRPC_VERSION, JsonRpcError, JsonRpcResponse};

pub(super) fn identity_from_runtime_alias(
    alias: &str,
) -> Option<crate::identity_first::AgentIdentity> {
    let alias = crate::member_comms_id::runtime_alias_str(alias);
    let rest = alias.strip_prefix("rt:")?;
    let (identity, generation) = rest.rsplit_once(':')?;
    if identity.is_empty()
        || generation.is_empty()
        || !generation.chars().all(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    crate::identity_first::AgentIdentity::parse(identity).ok()
}

/// The identity a stale runtime id names, with its registered status
/// (`None` when the identity runtime does not know it); `None` when the id
/// is no generated runtime id or is its identity's registered binding.
async fn stale_runtime_alias_detail(
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    alias: &str,
) -> Option<(
    crate::identity_first::AgentIdentity,
    Option<crate::identity_first::IdentityStatus>,
)> {
    let identity_runtime = identity_runtime?;
    let alias = crate::member_comms_id::runtime_alias_str(alias);
    let identity = identity_from_runtime_alias(alias.as_ref())?;
    let status = identity_runtime.status(&identity).await.ok();
    let registered = status
        .as_ref()
        .and_then(|status| status.agent_runtime_id.as_ref())
        .map(crate::identity_first::AgentRuntimeId::as_str);
    if registered == Some(alias.as_ref()) {
        return None;
    }
    Some((identity, status))
}

async fn stale_runtime_alias_error_response(
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    alias: &str,
    response_id: Value,
) -> Option<JsonRpcResponse> {
    let alias = crate::member_comms_id::runtime_alias_str(alias).into_owned();
    let (identity, status) = stale_runtime_alias_detail(identity_runtime, &alias).await?;
    Some(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(
            crate::identity_control_target::stale_identity_runtime_binding_error(
                "",
                &identity,
                status.as_ref(),
                &alias,
                None,
            ),
        ),
    })
}

async fn runtime_alias_is_stale(
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    alias: &str,
) -> bool {
    stale_runtime_alias_detail(identity_runtime, alias)
        .await
        .is_some()
}

fn runtime_binding_from_wire(
    binding: WireRuntimeBinding,
) -> Result<meerkat_mob::RuntimeBinding, String> {
    match binding {
        WireRuntimeBinding::Session => Ok(meerkat_mob::RuntimeBinding::Session),
        WireRuntimeBinding::External {
            address,
            bootstrap_token,
            identity,
        } => {
            let resolved = identity.resolve().map_err(|err| err.to_string())?;
            Ok(meerkat_mob::RuntimeBinding::External {
                peer_id: resolved.peer_id.to_string(),
                address,
                bootstrap_token,
                pubkey: resolved.pubkey,
            })
        }
    }
}

fn parse_optional_runtime_mode(params: &Value) -> Result<Option<MobRuntimeMode>, String> {
    match params.get("runtime_mode") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value::<MobRuntimeMode>(value.clone())
            .map(Some)
            .map_err(|err| {
                format!("runtime_mode must be \"autonomous_host\" or \"turn_driven\": {err}")
            }),
    }
}

fn parse_optional_backend(params: &Value) -> Result<Option<MobBackendKind>, String> {
    match params.get("backend") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value::<MobBackendKind>(value.clone())
            .map(Some)
            .map_err(|err| format!("backend must be \"session\" or \"external\": {err}")),
    }
}

fn parse_optional_runtime_binding(
    params: &Value,
) -> Result<Option<meerkat_mob::RuntimeBinding>, String> {
    match params.get("binding") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let wire = serde_json::from_value::<WireRuntimeBinding>(value.clone())
                .map_err(|err| format!("binding: {err}"))?;
            runtime_binding_from_wire(wire).map(Some)
        }
    }
}

pub(super) fn lifecycle_archive_cleanup_completed(error: &str) -> bool {
    is_recoverable_lifecycle_cleanup_error(error)
}

/// Parse HelperOptions from an optional JSON "options" object.
pub(crate) fn parse_helper_options(options_val: Option<&Value>) -> Result<HelperOptions, String> {
    let mut opts = HelperOptions::default();
    if let Some(o) = options_val {
        opts.role_name = o.get("role").and_then(Value::as_str).map(ProfileName::from);
        if let Some(mode_str) = o.get("runtime_mode").and_then(Value::as_str) {
            opts.runtime_mode = Some(
                serde_json::from_value::<MobRuntimeMode>(Value::String(mode_str.to_string()))
                    .map_err(|_| {
                        format!(
                            "invalid runtime_mode '{mode_str}': \
                             expected 'autonomous_host' or 'turn_driven'"
                        )
                    })?,
            );
        }
        if let Some(backend_str) = o.get("backend").and_then(Value::as_str) {
            opts.backend = Some(
                serde_json::from_value::<MobBackendKind>(Value::String(backend_str.to_string()))
                    .map_err(|_| format!("invalid backend '{backend_str}'"))?,
            );
        }
    }
    Ok(opts)
}

/// Extract content from params as `ContentInput`.
///
/// Accepts either:
/// - `"message": "plain text"` (string — backwards-compatible)
/// - `"content": "plain text"` (string shorthand)
/// - `"content": [{"type":"text","text":"..."},{"type":"image",...}]` (multimodal blocks)
///
/// `content` takes precedence if both are present so multipart-upload rewrites
/// cannot be shadowed by a stale text field. If `content` is present but
/// malformed, reject it instead of falling back to `message`.
fn extract_content(params: &Value) -> Result<Option<ContentInput>, String> {
    if let Some(content_val) = params.get("content") {
        return serde_json::from_value::<ContentInput>(content_val.clone())
            .map(Some)
            .map_err(|err| format!("invalid content: {err}"));
    }
    if let Some(s) = params.get("message").and_then(Value::as_str)
        && !s.is_empty()
    {
        return Ok(Some(ContentInput::Text(s.to_string())));
    }
    Ok(None)
}

fn content_input_to_console_value(content: &ContentInput) -> Value {
    match content {
        ContentInput::Text(text) => Value::String(text.clone()),
        ContentInput::Blocks(blocks) => serde_json::to_value(blocks).unwrap_or(Value::Null),
    }
}

/// Optional `handling_mode: "queue" | "steer"` JSON-RPC parameter.
/// Defaults to `Queue` when missing or null; unknown strings remain invalid.
pub(super) fn parse_handling_mode(
    params: &Value,
) -> Result<meerkat_core::types::HandlingMode, &'static str> {
    let Some(raw) = params.get("handling_mode") else {
        return Ok(meerkat_core::types::HandlingMode::Queue);
    };
    if raw.is_null() {
        return Ok(meerkat_core::types::HandlingMode::Queue);
    }
    match raw.as_str() {
        Some("queue") => Ok(meerkat_core::types::HandlingMode::Queue),
        Some("steer") => Ok(meerkat_core::types::HandlingMode::Steer),
        _ => Err("handling_mode must be \"queue\" or \"steer\""),
    }
}

/// Delivery target for `meta/send_message`, resolved from the wire
/// `member_id` parameter.
///
/// Precedence contract:
/// 1. A generated `rt:{identity}:{generation}` alias belongs to identity
///    authority and stays generation-fenced even while its roster row exists.
/// 2. An exact mob-roster match (after public-alias/comms encoding) keeps raw
///    member-id semantics unless the active `IdentityRuntime` binding proves
///    that the row is an identity-first embodiment. Identity-owned rows route
///    through `IdentityRuntime` so delivery preparation is never bypassed.
/// 3. Otherwise, on identity-first runtimes a bare durable identity (e.g.
///    `atlas-base-001` when the meerkat 0.7.1 roster holds
///    `rt:atlas-base-001:0`) resolves through the identity bridge —
///    the gateway-plane counterpart of the console plane's
///    `console_send_with_identity_first_fallback` (`http_console.rs`).
/// 4. Anything else (no identity runtime, unparseable identity, unknown
///    identity) falls through to raw member-id semantics so the original
///    mob `member not found` error surfaces unchanged.
enum SendMessageTarget {
    /// Deliver through the mob roster (`send_message_on_mob_with_interaction`).
    MobMember,
    /// Reserved generated aliases never degrade into the raw member plane.
    AuthorityUnavailable { alias: String },
    /// Deliver through the identity bridge
    /// (`IdentityRuntime::send_with_mode`), which lazily materializes
    /// dormant identities before delivery.
    Identity {
        identity_rt: std::sync::Arc<crate::identity_first::IdentityRuntime>,
        identity: crate::identity_first::AgentIdentity,
        /// Generated alias named by the caller. When present, validation and
        /// delivery stay under one identity lifecycle lock.
        expected_member_alias: Option<String>,
        /// Live runtime member id bound to the identity, when materialized.
        runtime_member_id: Option<String>,
        /// Bridge session bound to the identity at resolve time, when
        /// materialized. Pins the reported session reference: the post-send
        /// status re-read can race a concurrent retire/rebind, and an empty
        /// session id on the wire would read as a valid session reference to
        /// SDK typed results.
        session_id: Option<meerkat_core::types::SessionId>,
    },
}

/// Whether `status` reads the identity's own live member: Active, or Broken
/// while Core holds that live member Broken (the broken view still names its
/// runtime id). A roster row that is such a member routes through the
/// identity runtime, so delivery preparation is never bypassed. A broken
/// identity with no live member (a final wake refusal) and a dormant one do
/// not.
fn status_owns_live_member(status: &crate::identity_first::IdentityStatus) -> bool {
    match status.state {
        crate::identity_first::IdentityLifecycleState::Active => true,
        crate::identity_first::IdentityLifecycleState::Broken => status.agent_runtime_id.is_some(),
        crate::identity_first::IdentityLifecycleState::Dormant => false,
    }
}

async fn resolve_send_message_target(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    member_id: &str,
) -> SendMessageTarget {
    let member_id = crate::member_comms_id::runtime_alias_str(member_id).into_owned();
    let configured_identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    // Generated aliases belong to the identity authority even when the raw
    // member is still present in the roster. Taking roster precedence here
    // would reopen a reset-to-delivery TOCTOU on an old generation.
    if let Some(identity_rt) = configured_identity_runtime
        && let Some(identity) =
            crate::identity_first::IdentityRuntime::identity_for_generated_member_alias(&member_id)
    {
        let status = identity_rt.status(&identity).await.ok();
        return SendMessageTarget::Identity {
            identity_rt: identity_rt.clone(),
            identity,
            expected_member_alias: Some(member_id.clone()),
            runtime_member_id: status
                .as_ref()
                .and_then(|status| status.agent_runtime_id.as_ref())
                .map(|runtime_id| runtime_id.as_str().to_string()),
            session_id: status.and_then(|status| status.session_id),
        };
    }
    if crate::member_comms_id::is_reserved_generated_alias(&member_id) {
        return SendMessageTarget::AuthorityUnavailable { alias: member_id };
    }
    let roster_id = crate::member_comms_id::mob_member_id(&member_id);
    if let Ok(Some(member)) = runtime.mob_handle().get_member(&roster_id).await {
        if let Some(identity_rt) = configured_identity_runtime
            && let Ok(identity) = crate::identity_first::AgentIdentity::parse(&member_id)
            && crate::member_comms_id::live_member_is_identity(
                member.agent_identity.as_str(),
                identity.as_str(),
            )
            && let Ok(status) = identity_rt.status(&identity).await
            && status_owns_live_member(&status)
        {
            return SendMessageTarget::Identity {
                identity_rt: identity_rt.clone(),
                identity,
                expected_member_alias: None,
                runtime_member_id: status
                    .agent_runtime_id
                    .map(|runtime_id| runtime_id.as_str().to_string()),
                session_id: status.session_id,
            };
        }
        return SendMessageTarget::MobMember;
    }
    // Precedence pinning: the roster probe above is a point-in-time machine
    // command. A member declared in the reconcile baseline is a roster
    // member even when a concurrent retire/reconcile makes the probe read
    // absent (reconcile retires stale members before the replacement spawn
    // lands). Falling through to the identity bridge in that window would
    // silently deliver to a same-named durable identity's conversation;
    // keep raw member-id semantics instead, so the send either lands on the
    // (re)spawned member or surfaces the mob's own member-not-found error.
    if runtime
        .mob_runtime()
        .baseline_member_specs()
        .await
        .iter()
        .any(|spec| spec.identity.as_str() == member_id)
    {
        return SendMessageTarget::MobMember;
    }
    let Some(identity_rt) = configured_identity_runtime else {
        return SendMessageTarget::MobMember;
    };
    let Ok(identity) = crate::identity_first::AgentIdentity::parse(&member_id) else {
        return SendMessageTarget::MobMember;
    };
    match identity_rt.status(&identity).await {
        Ok(status) => SendMessageTarget::Identity {
            identity_rt: identity_rt.clone(),
            identity,
            expected_member_alias: None,
            runtime_member_id: status
                .agent_runtime_id
                .map(|runtime_id| runtime_id.as_str().to_string()),
            session_id: status.session_id,
        },
        // Unknown identity: keep raw member-id semantics so the original
        // mob `member not found` error surfaces.
        Err(_) => SendMessageTarget::MobMember,
    }
}

pub(super) async fn handle_send_message(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let member_id = params.get("member_id").and_then(Value::as_str);
    let content = match extract_content(params) {
        Ok(content) => content,
        Err(message) => {
            return JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message,
                    data: None,
                }),
            };
        }
    };
    let handling_mode = match parse_handling_mode(params) {
        Ok(mode) => mode,
        Err(message) => {
            return JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: message.to_string(),
                    data: None,
                }),
            };
        }
    };

    match (member_id, content) {
        (Some(member_id), Some(content)) if !member_id.is_empty() => {
            let member_id = crate::member_comms_id::runtime_alias_str(member_id).into_owned();
            if let Some(response) = stale_runtime_alias_error_response(
                identity_runtime,
                &member_id,
                response_id.clone(),
            )
            .await
            {
                return response;
            }
            let target = resolve_send_message_target(runtime, identity_runtime, &member_id).await;
            // Pre-flight the image-capability guard against the member that
            // will actually take the delivery: the wire id for roster sends,
            // the live runtime member for identity-bridge sends. A dormant
            // identity has no live member to project capabilities from yet —
            // it materializes inside `send_with_mode`, where the session
            // bridge re-runs the same guard.
            let image_guard_member_id = match &target {
                SendMessageTarget::MobMember => Some(member_id.as_str()),
                SendMessageTarget::AuthorityUnavailable { .. } => None,
                // Key the capability guard on the DURABLE identity: the roster
                // is keyed by its encoded form now, so the AgentRuntimeId would
                // resolve no member and the guard would silently pass. The
                // `runtime_member_id: Some(_)` requirement is retained, because
                // it is what limits the guard to an already-materialized
                // member; only the key changes.
                SendMessageTarget::Identity {
                    identity,
                    runtime_member_id: Some(_),
                    ..
                } => Some(identity.as_str()),
                SendMessageTarget::Identity { .. } => None,
            };
            if let Some(guard_member_id) = image_guard_member_id
                && let Err(err) = assert_member_accepts_images_with_config(
                    &runtime.mob_handle(),
                    runtime.mob_runtime().session_service(),
                    runtime.mob_runtime().member_model_config().as_deref(),
                    guard_member_id,
                    &content,
                )
                .await
            {
                return JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: err.to_string(),
                        data: None,
                    }),
                };
            }
            let content_value = content_input_to_console_value(&content);
            // A UUID reservation id rides the delivery as typed lineage (the
            // identity bridge threads it into the WorkSpec; the roster path
            // sets it on its own WorkSpec), so the run's terminal settles
            // exactly this reservation. A non-UUID id could never be threaded
            // and its reservation would never close.
            let interaction = meerkat_core::InteractionId::new();
            let interaction_id = interaction.to_string();
            let handling_mode_value = match handling_mode {
                meerkat_core::types::HandlingMode::Queue => "queue",
                meerkat_core::types::HandlingMode::Steer => "steer",
            };
            // Console events are keyed by the durable identity for
            // identity-bridge sends (mirroring the console plane) and by the
            // wire member id for roster sends. `reserve_interaction_value`'s
            // registration OVERWRITES `runtime_to_identity`, so the identity
            // argument must never be a generated `rt:{identity}:{generation}`
            // alias: self-mapping the incarnation clobbers the spawn path's
            // incarnation -> durable registration and re-keys every later
            // live event onto the incarnation conversation (the console
            // dispatch-mirroring defect). Generated aliases resolve to Identity
            // or AuthorityUnavailable before the roster probe. Raw workers keep
            // the MobMember arm, while active identity-owned roster rows route
            // through IdentityRuntime (both precedence cases are pinned below).
            let (events_identity, events_member_id) = match &target {
                SendMessageTarget::MobMember => (member_id.clone(), Some(member_id.clone())),
                SendMessageTarget::AuthorityUnavailable { alias } => (
                    crate::member_comms_id::durable_identity_from_runtime_alias(alias)
                        .unwrap_or_else(|| alias.clone()),
                    Some(alias.clone()),
                ),
                SendMessageTarget::Identity {
                    identity,
                    runtime_member_id,
                    ..
                } => (identity.as_str().to_string(), runtime_member_id.clone()),
            };
            let _ = runtime
                .console_events()
                .reserve_interaction_value(
                    &events_identity,
                    events_member_id.as_deref(),
                    &interaction_id,
                    "meta/send_message",
                    content_value.clone(),
                )
                .await;
            let delivery: Result<String, String> = match &target {
                SendMessageTarget::MobMember => send_message_on_mob_with_interaction(
                    &runtime.mob_handle(),
                    &member_id,
                    content.clone(),
                    handling_mode,
                    interaction,
                )
                .await
                .map_err(|err| err.to_string()),
                SendMessageTarget::Identity {
                    identity_rt,
                    identity,
                    expected_member_alias,
                    session_id: resolve_time_session_id,
                    ..
                } => match if let Some(expected_member_alias) = expected_member_alias {
                    identity_rt
                        .send_with_mode_and_interaction_member_alias_tracked(
                            identity,
                            expected_member_alias,
                            &content,
                            handling_mode,
                            Some(&interaction_id),
                        )
                        .await
                } else {
                    identity_rt
                        .send_with_mode_and_interaction_tracked(
                            identity,
                            &content,
                            handling_mode,
                            Some(&interaction_id),
                        )
                        .await
                } {
                    // Report the bridge session that took the delivery —
                    // re-read after the send so a lazy materialization or a
                    // delivered-session rebind is reflected, falling back to the
                    // resolve-time binding. NOTE: in the narrow race where BOTH
                    // the post-send re-read AND the resolve-time session are
                    // None (e.g. a dormant identity whose materialized session
                    // is concurrently retired/rebound in the send→re-read
                    // window), this yields an EMPTY string. The send still
                    // succeeded (`accepted: true`), so `session_id` may be
                    // empty on success; consumers must treat an empty
                    // `session_id` as "unknown", not as a usable reference.
                    Ok(_token) => Ok(identity_rt
                        .status(identity)
                        .await
                        .ok()
                        .and_then(|status| status.session_id)
                        .or_else(|| resolve_time_session_id.clone())
                        .map(|session_id| session_id.to_string())
                        .unwrap_or_default()),
                    Err(err) => Err(err.to_string()),
                },
                SendMessageTarget::AuthorityUnavailable { alias } => Err(format!(
                    "generated member alias requires current identity authority: {alias}"
                )),
            };
            match delivery {
                Ok(session_id) => {
                    runtime
                        .console_events()
                        .append(
                            &events_identity,
                            Some(interaction_id),
                            "user_input",
                            serde_json::json!({
                                "content": content_value,
                                "origin": "meta/send_message",
                                "session_id": session_id,
                                "handling_mode": handling_mode_value,
                            }),
                        )
                        .await;
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "accepted": true,
                            "member_id": member_id,
                            "session_id": session_id
                        })),
                        error: None,
                    }
                }
                Err(err) => {
                    runtime
                        .console_events()
                        .append(
                            &events_identity,
                            Some(interaction_id),
                            "interaction_failed",
                            serde_json::json!({
                                "reason": err,
                                "origin": "meta/send_message",
                                "content": content_value,
                            }),
                        )
                        .await;
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32000,
                            message: format!("send_message failed: {err}"),
                            data: None,
                        }),
                    }
                }
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: member_id and message (or content) required".to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_blob_get(
    runtime: &UnifiedRuntime,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let blob_id = params
        .get("blob_id")
        .or_else(|| params.get("id"))
        .and_then(Value::as_str);
    let Some(blob_id) = blob_id.filter(|value| !value.trim().is_empty()) else {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "blob_id required".to_string(),
                data: None,
            }),
        };
    };
    if !is_valid_blob_id_value(blob_id) {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "invalid blob_id".to_string(),
                data: None,
            }),
        };
    }
    let Some(store) = runtime.binary_blob_store() else {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: "binary blob store unavailable".to_string(),
                data: None,
            }),
        };
    };
    match store.get_bytes(&meerkat_core::BlobId::from(blob_id)).await {
        Ok(payload) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(serde_json::json!({
                "blob_id": payload.blob_id,
                "media_type": payload.media_type,
                "size": payload.size,
                "data": base64::engine::general_purpose::STANDARD.encode(payload.data.as_ref()),
            })),
            error: None,
        },
        Err(meerkat_core::BlobStoreError::NotFound(_)) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32001,
                message: format!("blob not found: {blob_id}"),
                data: Some(serde_json::json!({ "kind": "not_found", "blob_id": blob_id })),
            }),
        },
        Err(err) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: format!("blob get failed: {err}"),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_find_members(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let label_key = params.get("label_key").and_then(Value::as_str);
    let label_value = params.get("label_value").and_then(Value::as_str);

    match (label_key, label_value) {
        (Some(key), Some(value)) if !key.is_empty() => {
            let filter = MemberFilter {
                labels: std::collections::BTreeMap::from([(key.to_string(), value.to_string())]),
                role: None,
                status: None,
            };
            let handle = runtime.mob_handle();
            let entries = match handle.list_members_matching(filter).await {
                Ok(entries) => entries,
                Err(err) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32603,
                            message: format!("member lookup failed: {err}"),
                            data: None,
                        }),
                    };
                }
            };
            let mut members = Vec::with_capacity(entries.len());
            for entry in &entries {
                let alias =
                    crate::member_comms_id::runtime_alias_str(entry.agent_identity.as_str());
                if runtime_alias_is_stale(identity_runtime, alias.as_ref()).await {
                    continue;
                }
                members.push(member_entry_to_json(entry));
            }
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(Value::Array(members)),
                error: None,
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: label_key and label_value required".to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_ensure_member(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let role = params.get("role").and_then(Value::as_str);
    let agent_identity = params.get("agent_identity").and_then(Value::as_str);

    match (role, agent_identity) {
        (Some(role), Some(agent_identity)) if !role.is_empty() && !agent_identity.is_empty() => {
            let raw_reservation = match crate::member_comms_id::reserve_raw_member_target(
                identity_runtime,
                agent_identity,
            )
            .await
            {
                Ok(reservation) => reservation,
                Err(message) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {message}"),
                            data: None,
                        }),
                    };
                }
            };
            let labels = match params.get("labels") {
                None | Some(Value::Null) => None,
                Some(v) => {
                    match serde_json::from_value::<std::collections::BTreeMap<String, String>>(
                        v.clone(),
                    ) {
                        Ok(map) => Some(map),
                        Err(err) => {
                            return JsonRpcResponse {
                                jsonrpc: JSONRPC_VERSION.to_string(),
                                id: response_id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32602,
                                    message: format!(
                                        "Invalid params: labels must be a map of string to string: {err}"
                                    ),
                                    data: None,
                                }),
                            };
                        }
                    }
                }
            };
            if let Some(labels) = labels.as_ref()
                && let Err(message) = crate::member_comms_id::validate_raw_identity_labels(labels)
            {
                return JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {message}"),
                        data: None,
                    }),
                };
            }
            let context = params.get("context").cloned();
            let resume_session_id = match params.get("resume_session_id") {
                None | Some(Value::Null) => None,
                Some(v) => {
                    let s = match v.as_str() {
                        Some(s) => s,
                        None => {
                            return JsonRpcResponse {
                                jsonrpc: JSONRPC_VERSION.to_string(),
                                id: response_id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32602,
                                    message: "Invalid params: resume_session_id must be a string"
                                        .to_string(),
                                    data: None,
                                }),
                            };
                        }
                    };
                    match meerkat_core::types::SessionId::parse(s) {
                        Ok(sid) => Some(sid),
                        Err(_) => {
                            return JsonRpcResponse {
                                jsonrpc: JSONRPC_VERSION.to_string(),
                                id: response_id,
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32602,
                                    message: format!(
                                        "Invalid params: resume_session_id is not a valid session ID: {s}"
                                    ),
                                    data: None,
                                }),
                            };
                        }
                    }
                }
            };
            let additional_instructions = match params.get("additional_instructions") {
                None | Some(Value::Null) => None,
                Some(Value::Array(arr)) => {
                    let mut strs = Vec::with_capacity(arr.len());
                    for (i, entry) in arr.iter().enumerate() {
                        match entry.as_str() {
                            Some(s) => strs.push(s.to_string()),
                            None => {
                                return JsonRpcResponse {
                                    jsonrpc: JSONRPC_VERSION.to_string(),
                                    id: response_id,
                                    result: None,
                                    error: Some(JsonRpcError {
                                        code: -32602,
                                        message: format!(
                                            "Invalid params: additional_instructions[{i}] must be a string"
                                        ),
                                        data: None,
                                    }),
                                };
                            }
                        }
                    }
                    if strs.is_empty() { None } else { Some(strs) }
                }
                Some(_) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: "Invalid params: additional_instructions must be an array of strings".to_string(),
                    data: None,
                }),
                    };
                }
            };
            let runtime_mode = match parse_optional_runtime_mode(params) {
                Ok(value) => value,
                Err(message) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {message}"),
                            data: None,
                        }),
                    };
                }
            };
            let backend = match parse_optional_backend(params) {
                Ok(value) => value,
                Err(message) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {message}"),
                            data: None,
                        }),
                    };
                }
            };
            let binding = match parse_optional_runtime_binding(params) {
                Ok(value) => value,
                Err(message) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {message}"),
                            data: None,
                        }),
                    };
                }
            };

            let mut spec = SpawnMemberSpec::new(
                ProfileName::from(role),
                crate::member_comms_id::mob_member_id(raw_reservation.alias()),
            );
            if let Some(runtime_mode) = runtime_mode {
                spec = spec.with_runtime_mode(runtime_mode);
            }
            if let Some(backend) = backend {
                spec = spec.with_backend(backend);
            }
            if let Some(binding) = binding {
                spec.binding = Some(binding);
            }
            if let Some(context) = context {
                spec = spec.with_context(context);
            }
            if let Some(labels) = labels {
                spec = spec.with_labels(labels);
            }
            if let Some(sid) = resume_session_id {
                spec = spec.with_resume_bridge_session_id(sid);
            }
            if let Some(instructions) = additional_instructions {
                spec = spec.with_additional_instructions(instructions);
            }
            let handle = runtime.mob_handle();
            let mid = spec.identity.clone();
            let ensure_result = handle.ensure_member(spec).await;
            drop(raw_reservation);
            match ensure_result {
                Ok(_outcome) => {
                    // Declared definition wiring (auto_wire_orchestrator /
                    // role_wiring) is bring-up-order dependent at spawn time
                    // upstream; reconcile after every ensure so the crew
                    // topology converges regardless of the order hosts
                    // materialize members in. Inert without an edge policy;
                    // idempotent otherwise. An incomplete reconcile pages and
                    // logs each failed edge operation inside
                    // `reconcile_edges`, so the report needs nothing here.
                    let _ = runtime.reconcile_edges().await;
                    let entries = handle.list_members_including_retiring().await;
                    let entry = entries.into_iter().find(|e| e.agent_identity == mid);
                    let result = match entry {
                        Some(e) => member_entry_to_json(&e),
                        None => Value::Null,
                    };
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(result),
                        error: None,
                    }
                }
                Err(err) => {
                    match crate::identity_first::bridge::non_durable_service_refusal(&err) {
                        // A durable profile on a mob whose session service keeps
                        // no persistent sessions: meerkat's closed refusal keeps
                        // its own code and data (`CAPABILITY_UNAVAILABLE`,
                        // `data.kind = "mob_non_durable_service"`), so a caller
                        // can tell it from a retryable failure.
                        Some((refusal, _)) => mob_declaration_error(response_id, refusal),
                        None => JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32602,
                                message: format!("ensure_member failed: {err}"),
                                data: None,
                            }),
                        },
                    }
                }
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: role and agent_identity required".to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_list_members(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let handle = runtime.mob_handle();
    let entries = handle.list_members_including_retiring().await;
    let mut members = Vec::with_capacity(entries.len());
    for entry in &entries {
        let alias = crate::member_comms_id::runtime_alias_str(entry.agent_identity.as_str());
        if runtime_alias_is_stale(identity_runtime, alias.as_ref()).await {
            continue;
        }
        members.push(member_entry_to_json(entry));
    }
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: Some(Value::Array(members)),
        error: None,
    }
}

pub(super) async fn handle_get_member(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let member_id = params.get("member_id").and_then(Value::as_str);
    match member_id {
        Some(mid) if !mid.is_empty() => {
            let mid = crate::member_comms_id::runtime_alias_str(mid).into_owned();
            if let Some(response) =
                stale_runtime_alias_error_response(identity_runtime, &mid, response_id.clone())
                    .await
            {
                return response;
            }
            let handle = runtime.mob_handle();
            // Decode before encoding: a caller may hand back the runtime alias
            // `status_identity` returns, and encoding that directly yields a
            // well-formed roster id for a member that does not exist.
            let identity = crate::member_comms_id::roster_member_id_for_supplied_id(&mid);
            let entries = handle.list_members_including_retiring().await;
            match entries.into_iter().find(|e| e.agent_identity == identity) {
                Some(entry) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(member_entry_to_json(&entry)),
                    error: None,
                },
                None => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("member not found: {mid}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: member_id required".to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_retire_member(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let member_id = params.get("member_id").and_then(Value::as_str);
    match member_id {
        Some(mid) if !mid.is_empty() => {
            let mid = crate::member_comms_id::runtime_alias_str(mid).into_owned();
            if let Some(response) =
                stale_runtime_alias_error_response(identity_runtime, &mid, response_id.clone())
                    .await
            {
                return response;
            }
            // A current runtime alias owned by the durable identity plane is
            // not stale, but it still must not fall through to the raw Mob
            // handle. Retire through IdentityRuntime so fencing, continuity,
            // bridge state, and shutdown ownership advance together.
            if let Some(identity_runtime) = identity_runtime
                && let Some(durable) = identity_runtime.identity_for_member_mutation(&mid).await
            {
                return match identity_runtime
                    .retire_member_alias_tracked(&durable, &mid)
                    .await
                {
                    Ok(()) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "accepted": true,
                            "identity_first": true,
                        })),
                        error: None,
                    },
                    Err(err) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32000,
                            message: format!("retire_member (identity) failed: {err}"),
                            data: None,
                        }),
                    },
                };
            }
            if crate::member_comms_id::is_reserved_generated_alias(&mid) {
                return JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!(
                            "generated member alias requires current identity authority: {mid}"
                        ),
                        data: None,
                    }),
                };
            }
            let handle = runtime.mob_handle();
            match handle
                .retire(crate::member_comms_id::mob_member_id(&mid))
                .await
            {
                Ok(()) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({"accepted": true})),
                    error: None,
                },
                Err(err) if lifecycle_archive_cleanup_completed(&err.to_string()) => {
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({"accepted": true})),
                        error: None,
                    }
                }
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("retire_member failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: member_id required".to_string(),
                data: None,
            }),
        },
    }
}

/// `meta/reload_member`: non-destructive cold reload of an identity's live
/// member (same identity, same durable session, same continuity generation).
/// Identity-plane only: the worker plane's `respawn` is a destructive reset
/// and is never called from here.
pub(super) async fn handle_reload_member(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let Some(mid) = params
        .get("member_id")
        .and_then(Value::as_str)
        .filter(|mid| !mid.is_empty())
    else {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: member_id required".to_string(),
                data: None,
            }),
        };
    };
    let mid = crate::member_comms_id::runtime_alias_str(mid).into_owned();
    if let Some(response) =
        stale_runtime_alias_error_response(identity_runtime, &mid, response_id.clone()).await
    {
        return response;
    }
    let Some(identity_runtime) = identity_runtime else {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: "reload_member requires identity-first runtime authority; the worker \
                          plane has no non-destructive reload (respawn_member is a destructive \
                          reset)"
                    .to_string(),
                data: None,
            }),
        };
    };
    let Some(durable) = identity_runtime.identity_for_member_mutation(&mid).await else {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32001,
                message: format!("unknown identity: {mid}"),
                data: None,
            }),
        };
    };
    let expected_alias =
        crate::member_comms_id::is_reserved_generated_alias(&mid).then_some(mid.as_str());
    match identity_runtime
        .reload_member_alias_tracked(&durable, expected_alias)
        .await
    {
        Ok(outcome) => {
            let mut body = serde_json::to_value(&outcome).unwrap_or(Value::Null);
            body["identity_first"] = Value::Bool(true);
            body["identity"] = Value::String(durable.to_string());
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(body),
                error: None,
            }
        }
        Err(err) => super::identity_error_response(response_id, &err),
    }
}

/// `meta/member_health`: the identity's lifecycle and delivery health from
/// in-process reads only (never touches the mob actor, so it answers while
/// the loop is stalled).
pub(super) async fn handle_member_health(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let Some(mid) = params
        .get("member_id")
        .or_else(|| params.get("identity"))
        .and_then(Value::as_str)
        .filter(|mid| !mid.is_empty())
    else {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: member_id required".to_string(),
                data: None,
            }),
        };
    };
    let mid = crate::member_comms_id::runtime_alias_str(mid).into_owned();
    if let Some(response) =
        stale_runtime_alias_error_response(identity_runtime, &mid, response_id.clone()).await
    {
        return response;
    }
    let Some(identity_runtime) = identity_runtime else {
        return JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: "member_health requires identity-first runtime authority".to_string(),
                data: None,
            }),
        };
    };
    let identity = match identity_runtime.identity_for_member_mutation(&mid).await {
        Some(identity) => identity,
        None => match crate::identity_first::AgentIdentity::parse(&mid) {
            Ok(identity) => identity,
            Err(_) => {
                return JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32001,
                        message: format!("unknown identity: {mid}"),
                        data: None,
                    }),
                };
            }
        },
    };
    match identity_runtime.member_health(&identity).await {
        Ok(report) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(serde_json::to_value(&report).unwrap_or(Value::Null)),
            error: None,
        },
        Err(err) => super::identity_error_response(response_id, &err),
    }
}

pub(super) async fn handle_respawn_member(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let member_id = params.get("member_id").and_then(Value::as_str);
    match member_id {
        Some(mid) if !mid.is_empty() => {
            let mid = crate::member_comms_id::runtime_alias_str(mid).into_owned();
            if let Some(response) =
                stale_runtime_alias_error_response(identity_runtime, &mid, response_id.clone())
                    .await
            {
                return response;
            }
            // Doctrine: identity-owned members respawn through the identity
            // authority — reset rebuilds a fresh session under the SAME
            // durable identity (new generation, continuity preserved).
            if let Some(identity_runtime) = identity_runtime
                && let Some(durable) = identity_runtime.identity_for_member_mutation(&mid).await
            {
                return match identity_runtime
                    .reset_member_alias_tracked(&durable, &mid)
                    .await
                {
                    Ok(binding) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "accepted": true,
                            "identity_first": true,
                            "session_id": binding.session_id().map(ToString::to_string),
                            "generation": binding.generation().get(),
                        })),
                        error: None,
                    },
                    Err(err) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32000,
                            message: format!("respawn_member (identity) failed: {err}"),
                            data: None,
                        }),
                    },
                };
            }
            if crate::member_comms_id::is_reserved_generated_alias(&mid) {
                return JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!(
                            "generated member alias requires current identity authority: {mid}"
                        ),
                        data: None,
                    }),
                };
            }
            let handle = runtime.mob_handle();
            // Decode before encoding: a caller may hand back the runtime alias
            // `status_identity` returns, and encoding that directly yields a
            // well-formed roster id for a member that does not exist.
            let identity = crate::member_comms_id::roster_member_id_for_supplied_id(&mid);
            // Best-effort repair material: a faulted lookup degrades to None
            // (the respawn itself surfaces real faults).
            let entry_before_respawn = handle.get_member(&identity).await.ok().flatten();
            let mut topology_restore_warning = None;
            let idle_retire_overrides = runtime
                .mob_runtime()
                .implicit_delegate_retirement_overrides();
            match crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
                idle_retire_overrides.as_ref(),
                &handle,
                &identity,
                handle.respawn(identity.clone(), None),
                Result::is_ok,
            )
            .await
            {
                Ok(_receipt) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({"accepted": true})),
                    error: None,
                },
                Err(err) => {
                    if let Some(failed_peer_ids) = topology_restore_failed_peer_ids(&err) {
                        tracing::warn!(
                            member_id = %identity,
                            failed_peer_count = failed_peer_ids.len(),
                            failed_peer_ids = ?failed_peer_ids,
                            "rpc member respawn restored member with isolated peer edges; accepting degraded respawn"
                        );
                        topology_restore_warning =
                            Some(topology_restore_warning_json(&failed_peer_ids));
                    } else if lifecycle_archive_cleanup_completed(&err.to_string()) {
                        // A faulted lookup must not read as "absent" (that
                        // would mint a spurious replacement member).
                        let member_after_cleanup = match handle.get_member(&identity).await {
                            Ok(member) => member,
                            Err(lookup_err) => {
                                return JsonRpcResponse {
                                    jsonrpc: JSONRPC_VERSION.to_string(),
                                    id: response_id,
                                    result: None,
                                    error: Some(JsonRpcError {
                                        code: -32000,
                                        message: format!("respawn_member failed: {lookup_err}"),
                                        data: None,
                                    }),
                                };
                            }
                        };
                        if member_after_cleanup.is_none()
                            && let Some(entry) = entry_before_respawn
                        {
                            let mut spec =
                                SpawnMemberSpec::new(entry.role.clone(), identity.clone());
                            if !entry.labels.is_empty() {
                                spec = spec.with_labels(entry.labels.clone());
                            }
                            if let Err(ensure_err) = handle.ensure_member(spec).await {
                                return JsonRpcResponse {
                                    jsonrpc: JSONRPC_VERSION.to_string(),
                                    id: response_id,
                                    result: None,
                                    error: Some(JsonRpcError {
                                        code: -32000,
                                        message: format!("respawn_member failed: {ensure_err}"),
                                        data: None,
                                    }),
                                };
                            }
                        }
                    } else {
                        return JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32000,
                                message: format!("respawn_member failed: {err}"),
                                data: None,
                            }),
                        };
                    }

                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "accepted": true,
                            "topology_restore_warning": topology_restore_warning,
                        })),
                        error: None,
                    }
                }
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: member_id required".to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_reconcile_edges(
    runtime: &UnifiedRuntime,
    response_id: Value,
) -> JsonRpcResponse {
    // A candidate launch's reconcile is a dry run, flagged `applied: false`.
    let report = runtime.reconcile_edges().await;
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: Some(
            crate::unified_runtime::edge_reconcile::reconcile_edges_result(
                &report,
                runtime.speaks_for_composition(),
            ),
        ),
        error: None,
    }
}

pub(super) async fn handle_rediscover(
    runtime: &UnifiedRuntime,
    response_id: Value,
) -> JsonRpcResponse {
    match runtime.rediscover().await {
        Ok(Some(report)) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(serde_json::to_value(&report).unwrap_or(Value::Null)),
            error: None,
        },
        Ok(None) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(serde_json::json!({
                "status": "no_discovery_configured"
            })),
            error: None,
        },
        Err(err) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: format!("rediscover failed: {err}"),
                data: None,
            }),
        },
    }
}

fn parse_mob_events_query(
    response_id: &Value,
    params: Value,
) -> Result<crate::unified_runtime::EventQuery, Box<JsonRpcResponse>> {
    if params.is_null() {
        return Ok(crate::unified_runtime::EventQuery::default());
    }
    serde_json::from_value(params).map_err(|err| {
        Box::new(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id.clone(),
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: format!("Invalid params: {err}"),
                data: None,
            }),
        })
    })
}

use super::MOB_EVENTS_STALE_CURSOR_CODE;

fn stale_cursor_response(
    response_id: Value,
    after_cursor: u64,
    latest_cursor: u64,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: MOB_EVENTS_STALE_CURSOR_CODE,
            message: format!(
                "stale mob event cursor: requested {after_cursor}, latest {latest_cursor}"
            ),
            data: Some(serde_json::json!({
                "error": "event_query_stale",
                "after_cursor": after_cursor,
                "latest_cursor": latest_cursor,
            })),
        }),
    }
}

/// Handle `meta/mob_events/query` — return structural mob events
/// matching the supplied [`EventQuery`] by scanning the meerkat ledger.
pub(super) async fn handle_mob_events_query(
    runtime: &UnifiedRuntime,
    response_id: Value,
    params: Value,
) -> JsonRpcResponse {
    let query = match parse_mob_events_query(&response_id, params) {
        Ok(q) => q,
        Err(response) => return *response,
    };
    // Capture latest_cursor up front so an empty result still yields a
    // numeric `next_after_seq`. Pre-fix, an empty match returned
    // `next_after_seq: null` and a polling SDK had no anchor to resume
    // from — it would either restart from latest (skipping events) or
    // 0 (replaying everything).
    let fallback_cursor = runtime
        .mob_handle()
        .events()
        .latest_cursor()
        .await
        .unwrap_or(0);
    match runtime.query_mob_events(&query).await {
        Ok(events) => {
            let next_after_seq = events
                .last()
                .map(|event| event.cursor)
                .or(query.after_seq)
                .unwrap_or(fallback_cursor);
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({
                    "events": serde_json::to_value(&events).unwrap_or(Value::Null),
                    "next_after_seq": next_after_seq,
                })),
                error: None,
            }
        }
        Err(crate::unified_runtime::mob_events::MobEventsQueryError::Stale {
            after_cursor,
            latest_cursor,
        }) => stale_cursor_response(response_id, after_cursor, latest_cursor),
        Err(err) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: format!("mob_events/query failed: {err}"),
                data: None,
            }),
        },
    }
}

/// Handle `meta/mob_events/subscribe` — JSON-RPC handshake that
/// returns a snapshot of structural events plus a `subscribe_url`
/// pointing to the SSE route. Pre-handshake validation rejects stale
/// cursors with the typed `-32010` error before any SSE connection is
/// opened.
pub(super) async fn handle_mob_events_subscribe(
    runtime: &UnifiedRuntime,
    response_id: Value,
    params: Value,
) -> JsonRpcResponse {
    let query = match parse_mob_events_query(&response_id, params) {
        Ok(q) => q,
        Err(response) => return *response,
    };
    // Capture latest_cursor at handshake so the continuation URL
    // covers the empty-snapshot case (no events matched yet, no caller
    // after_seq supplied) without dropping a window between snapshot
    // and SSE connect.
    let latest_at_handshake = runtime
        .mob_handle()
        .events()
        .latest_cursor()
        .await
        .unwrap_or(0);
    match runtime.query_mob_events(&query).await {
        Ok(events) => {
            let last_cursor = events.last().map(|event| event.cursor);
            let subscribe_url = crate::unified_runtime::mob_events::build_subscribe_url(
                &query,
                last_cursor,
                latest_at_handshake,
            );
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({
                    "stream": "mob_events",
                    "events": serde_json::to_value(&events).unwrap_or(Value::Null),
                    "next_after_seq": last_cursor,
                    "subscribe_url": subscribe_url,
                    "keep_alive": {
                        "interval_ms": 15_000_u64,
                        "event": "keep_alive",
                    },
                })),
                error: None,
            }
        }
        Err(crate::unified_runtime::mob_events::MobEventsQueryError::Stale {
            after_cursor,
            latest_cursor,
        }) => stale_cursor_response(response_id, after_cursor, latest_cursor),
        Err(err) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: format!("mob_events/subscribe failed: {err}"),
                data: None,
            }),
        },
    }
}

// ---------------------------------------------------------------------------
// Cross-mob operations
// ---------------------------------------------------------------------------

async fn run_cross_mob_local_member_operation<T, F, Fut, G, GFut>(
    runtime: &UnifiedRuntime,
    runtime_owner: Option<std::sync::Arc<UnifiedRuntime>>,
    _identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    _local_member_id: &str,
    owned_operation: F,
    borrowed_operation: G,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(std::sync::Arc<UnifiedRuntime>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send + 'static,
    G: FnOnce() -> GFut,
    GFut: std::future::Future<Output = Result<T, String>>,
{
    // UnifiedRuntime owns both local and remote identity admission at each
    // concrete member-plane mutation. Keeping the RPC layer out of the
    // lifecycle lock avoids recursively acquiring the same per-identity lock.
    if let Some(runtime_owner) = runtime_owner {
        return owned_operation(runtime_owner).await;
    }
    let _ = runtime;
    borrowed_operation().await
}

pub(super) async fn handle_cross_mob_wire(
    runtime: &UnifiedRuntime,
    runtime_owner: Option<std::sync::Arc<UnifiedRuntime>>,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let local_member_id = params.get("local_member_id").and_then(Value::as_str);
    let remote_member_id = params.get("remote_member_id").and_then(Value::as_str);
    let remote_mob_id = params.get("remote_mob_id").and_then(Value::as_str);

    match (local_member_id, remote_member_id, remote_mob_id) {
        (Some(local), Some(remote), Some(mob))
            if !local.is_empty() && !remote.is_empty() && !mob.is_empty() =>
        {
            let owned_local = local.to_string();
            let owned_remote = remote.to_string();
            let owned_mob = mob.to_string();
            let owned_identity_runtime = identity_runtime.cloned();
            let borrowed_identity_runtime = identity_runtime.cloned();
            match run_cross_mob_local_member_operation(
                runtime,
                runtime_owner,
                identity_runtime,
                local,
                move |runtime| async move {
                    Box::pin(runtime.wire_cross_mob_with_identity_runtime(
                        &owned_local,
                        &owned_remote,
                        &owned_mob,
                        owned_identity_runtime.as_ref(),
                    ))
                    .await
                    .map_err(|err| err.to_string())
                },
                || async move {
                    Box::pin(runtime.wire_cross_mob_with_identity_runtime(
                        local,
                        remote,
                        mob,
                        borrowed_identity_runtime.as_ref(),
                    ))
                    .await
                    .map_err(|err| err.to_string())
                },
            )
            .await
            {
                Ok(()) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "accepted": true,
                        "local_member_id": local,
                        "remote_member_id": remote,
                        "remote_mob_id": mob,
                    })),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("cross_mob/wire failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message:
                    "Invalid params: local_member_id, remote_member_id, and remote_mob_id required"
                        .to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_cross_mob_unwire(
    runtime: &UnifiedRuntime,
    runtime_owner: Option<std::sync::Arc<UnifiedRuntime>>,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let local_member_id = params.get("local_member_id").and_then(Value::as_str);
    let remote_member_id = params.get("remote_member_id").and_then(Value::as_str);
    let remote_mob_id = params.get("remote_mob_id").and_then(Value::as_str);

    match (local_member_id, remote_member_id, remote_mob_id) {
        (Some(local), Some(remote), Some(mob))
            if !local.is_empty() && !remote.is_empty() && !mob.is_empty() =>
        {
            let owned_local = local.to_string();
            let owned_remote = remote.to_string();
            let owned_mob = mob.to_string();
            let owned_identity_runtime = identity_runtime.cloned();
            let borrowed_identity_runtime = identity_runtime.cloned();
            match run_cross_mob_local_member_operation(
                runtime,
                runtime_owner,
                identity_runtime,
                local,
                move |runtime| async move {
                    Box::pin(runtime.unwire_cross_mob_with_identity_runtime(
                        &owned_local,
                        &owned_remote,
                        &owned_mob,
                        owned_identity_runtime.as_ref(),
                    ))
                    .await
                    .map_err(|err| err.to_string())
                },
                || async move {
                    Box::pin(runtime.unwire_cross_mob_with_identity_runtime(
                        local,
                        remote,
                        mob,
                        borrowed_identity_runtime.as_ref(),
                    ))
                    .await
                    .map_err(|err| err.to_string())
                },
            )
            .await
            {
                Ok(()) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "accepted": true,
                        "local_member_id": local,
                        "remote_member_id": remote,
                        "remote_mob_id": mob,
                    })),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("cross_mob/unwire failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message:
                    "Invalid params: local_member_id, remote_member_id, and remote_mob_id required"
                        .to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_cross_mob_send(
    runtime: &UnifiedRuntime,
    runtime_owner: Option<std::sync::Arc<UnifiedRuntime>>,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let from_member_id = params.get("from_member_id").and_then(Value::as_str);
    let remote_member_id = params.get("remote_member_id").and_then(Value::as_str);
    let remote_mob_id = params.get("remote_mob_id").and_then(Value::as_str);
    let content = match extract_content(params) {
        Ok(content) => content,
        Err(message) => {
            return JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message,
                    data: None,
                }),
            };
        }
    };

    match (from_member_id, remote_member_id, remote_mob_id, content) {
        (Some(from), Some(remote), Some(mob), Some(content))
            if !from.is_empty() && !remote.is_empty() && !mob.is_empty() =>
        {
            let owned_from = from.to_string();
            let owned_remote = remote.to_string();
            let owned_mob = mob.to_string();
            let owned_content = content.clone();
            let owned_identity_runtime = identity_runtime.cloned();
            let borrowed_identity_runtime = identity_runtime.cloned();
            match run_cross_mob_local_member_operation(
                runtime,
                runtime_owner,
                identity_runtime,
                from,
                move |runtime| async move {
                    runtime
                        .send_cross_mob_with_identity_runtime(
                            &owned_from,
                            &owned_remote,
                            &owned_mob,
                            owned_content,
                            owned_identity_runtime.as_ref(),
                        )
                        .await
                        .map_err(|err| err.to_string())
                },
                || async move {
                    runtime
                        .send_cross_mob_with_identity_runtime(
                            from,
                            remote,
                            mob,
                            content,
                            borrowed_identity_runtime.as_ref(),
                        )
                        .await
                        .map_err(|err| err.to_string())
                },
            )
            .await
            {
                Ok(session_id) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "accepted": true,
                        "from_member_id": from,
                        "remote_member_id": remote,
                        "remote_mob_id": mob,
                        "session_id": session_id,
                    })),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        // Context, not a verdict. A leading "failed" overrides
                        // the inner classification for every reader - including
                        // a model-mediated one - and would tell a caller to
                        // retry an ambiguous delivery, which is unsafe. The
                        // inner error already states its own outcome.
                        message: format!("cross_mob/send: {err}"),
                    data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: from_member_id, remote_member_id, remote_mob_id, and message (or content) required".to_string(),
                    data: None,
                }),
        },
    }
}

pub(super) async fn handle_cross_mob_directory(
    runtime: &UnifiedRuntime,
    response_id: Value,
) -> JsonRpcResponse {
    let entries: Vec<Value> = runtime
        .list_external_mobs()
        .into_iter()
        .filter_map(|e| serde_json::to_value(&e).ok())
        .collect();
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: Some(serde_json::json!({ "mobs": entries })),
        error: None,
    }
}

/// Return comms peer info for a local member — used by the Python SDK
/// to build `TrustedPeerSpec` for cross-mob wiring.
pub(super) async fn handle_cross_mob_peer_info(
    runtime: &UnifiedRuntime,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let member_id = params.get("member_id").and_then(Value::as_str);
    match member_id {
        Some(mid) if !mid.is_empty() => match runtime.local_member_peer_info(mid).await {
            Ok((peer_id, comms_name, address)) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({
                    "member_id": mid,
                    "mob_id": runtime.mob_id(),
                    "comms_name": comms_name,
                    "peer_id": peer_id,
                    "address": address,
                })),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32000,
                    message: format!("cross_mob/peer_info failed: {err}"),
                    data: None,
                }),
            },
        },
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: member_id required".to_string(),
                data: None,
            }),
        },
    }
}

// ---------------------------------------------------------------------------
// 0.5 API surface handlers
// ---------------------------------------------------------------------------

pub(super) async fn handle_member_status(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let member_id = params.get("member_id").and_then(Value::as_str);
    match member_id {
        Some(mid) if !mid.is_empty() => {
            let mid = crate::member_comms_id::runtime_alias_str(mid).into_owned();
            if let Some(response) =
                stale_runtime_alias_error_response(identity_runtime, &mid, response_id.clone())
                    .await
            {
                return response;
            }
            // Decode before encoding, as `handle_get_member` does: the current
            // runtime alias `status_identity` returns names the durable roster
            // row, and encoding it directly reads a member that does not exist.
            match runtime
                .mob_handle()
                .member_status(&crate::member_comms_id::roster_member_id_for_supplied_id(
                    &mid,
                ))
                .await
            {
                Ok(snapshot) => {
                    let mut result = serde_json::to_value(&snapshot).unwrap_or(Value::Null);
                    if let Some(session_id) = snapshot.current_session_id.as_ref()
                        && let Some(job_health) = runtime.job_health_projection()
                        && let Some(session_jobs) = job_health
                            .get("by_session")
                            .and_then(|by_session| by_session.get(session_id.to_string()))
                    {
                        result["detached_jobs"] = session_jobs.clone();
                        result["awaiting_detached"] = session_jobs
                            .get("awaiting_detached")
                            .cloned()
                            .unwrap_or(Value::Bool(false));
                    }
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(result),
                        error: None,
                    }
                }
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("member_status failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: member_id required".to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_identity_resolved_tools(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity = params
        .get("identity")
        .or_else(|| params.get("member_id"))
        .and_then(Value::as_str);
    match identity {
        Some(identity) if !identity.is_empty() => {
            let identity = crate::member_comms_id::runtime_alias_str(identity).into_owned();
            let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
            if let Some(response) =
                stale_runtime_alias_error_response(identity_runtime, &identity, response_id.clone())
                    .await
            {
                return response;
            }
            if let Some(identity_runtime) = identity_runtime
                && let Ok(parsed) = crate::identity_first::AgentIdentity::parse(&identity)
                && let Ok(status) = identity_runtime.status(&parsed).await
                && let Some(session_id) = status.session_id
            {
                match resolved_tools_for_session(
                    runtime.mob_runtime().session_service(),
                    &identity,
                    session_id,
                )
                .await
                {
                    Ok(snapshot) => {
                        return JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: Some(serde_json::to_value(&snapshot).unwrap_or(Value::Null)),
                            error: None,
                        };
                    }
                    Err(err) => {
                        return JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32000,
                                message: format!("resolved_tools failed: {err}"),
                                data: None,
                            }),
                        };
                    }
                }
            }
            match resolved_tools_for_member(
                &runtime.mob_handle(),
                runtime.mob_runtime().session_service(),
                &identity,
            )
            .await
            {
                Ok(snapshot) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(&snapshot).unwrap_or(Value::Null)),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("resolved_tools failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: identity required".to_string(),
                data: None,
            }),
        },
    }
}

/// Render a routing-status failure with a machine-readable `reason` so a caller
/// sweeping a fleet can classify each identity instead of only failing it.
///
/// `no_current_session` is the expected state for a materialized identity
/// that has never been addressed, and must stay distinguishable from
/// `session_not_held`, which is a genuine defect.
fn routing_status_error_response(
    response_id: Value,
    identity: &str,
    err: &crate::mob_handle_runtime::RoutingStatusUnavailable,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: routing_status_error_code(err),
            message: format!("routing_status unavailable: {err}"),
            data: Some(routing_status_error_data(identity, err)),
        }),
    }
}

/// A malformed request is `-32602` (invalid params); everything else is a
/// server-side `-32000`. The two codes carry the SAME `data` shape on purpose,
/// so a caller branches on `data.reason` uniformly and never has to pair a
/// reason with a code to interpret it.
pub(crate) fn routing_status_error_code(
    err: &crate::mob_handle_runtime::RoutingStatusUnavailable,
) -> i64 {
    match err {
        crate::mob_handle_runtime::RoutingStatusUnavailable::InvalidIdentity(_) => -32602,
        _ => -32000,
    }
}

pub(crate) fn routing_status_error_data(
    identity: &str,
    err: &crate::mob_handle_runtime::RoutingStatusUnavailable,
) -> Value {
    serde_json::json!({
        "kind": "routing_status_unavailable",
        "reason": err.reason_code(),
        "identity": identity,
    })
}

/// ONE owner of "is this request's identity parameter usable", shared by both
/// dispatch planes.
///
/// Previously each plane rejected a missing or empty identity inline with an
/// untyped `-32602` whose `data` was `None`, and the two planes did not even
/// agree with each other. The consequence was worse than inconsistency:
/// `invalid_identity` was documented as a member of the closed reason set and
/// was UNREACHABLE through either surface, because both handlers refused before
/// anything could construct it. A reason a caller can never observe is not part
/// of a contract.
///
/// Returns the runtime-alias-decoded identity on success. Empty and missing are
/// distinguished by message but share the `invalid_identity` reason, because
/// what a caller does about them is the same.
pub(crate) fn routing_status_identity_param(
    params: &Value,
) -> Result<String, (String, crate::mob_handle_runtime::RoutingStatusUnavailable)> {
    use crate::mob_handle_runtime::RoutingStatusUnavailable;
    let supplied = params
        .get("identity")
        .or_else(|| params.get("member_id"))
        .and_then(Value::as_str);
    match supplied {
        Some(identity) if !identity.trim().is_empty() => {
            Ok(crate::member_comms_id::runtime_alias_str(identity).into_owned())
        }
        // Echo back exactly what was supplied, including the empty string, so
        // the caller can tell "I sent an empty identity" from "I sent none".
        Some(identity) => Err((
            identity.to_string(),
            RoutingStatusUnavailable::InvalidIdentity("identity must not be empty"),
        )),
        None => Err((
            String::new(),
            RoutingStatusUnavailable::InvalidIdentity("identity required"),
        )),
    }
}

/// `meta/identity/routing_status` - meerkat's typed per-session model
/// routing status, addressed by Meta identity.
///
/// Structurally identical to [`handle_identity_resolved_tools`]: resolve the
/// identity through the identity runtime when one is bound, otherwise fall
/// back to the mob handle's member status. What differs is the failure
/// posture. Routing status is a machine read, and meerkat returns `NotFound`
/// for a session its runtime machine does not hold; that error is propagated
/// verbatim instead of being softened into an empty status, because an empty
/// status is indistinguishable on the wire from a legitimately pre-hydration
/// one (see `model_routing_status_for_session`).
pub(super) async fn handle_identity_routing_status(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity = match routing_status_identity_param(params) {
        Ok(identity) => identity,
        Err((supplied, err)) => {
            return routing_status_error_response(response_id, &supplied, &err);
        }
    };
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    if let Some(response) =
        stale_runtime_alias_error_response(identity_runtime, &identity, response_id.clone()).await
    {
        return response;
    }
    if let Some(identity_runtime) = identity_runtime
        && let Ok(parsed) = crate::identity_first::AgentIdentity::parse(&identity)
        && let Ok(status) = identity_runtime.status(&parsed).await
        && let Some(session_id) = status.session_id
    {
        return match model_routing_status_for_session(
            runtime.mob_runtime().session_service(),
            &identity,
            session_id,
        )
        .await
        {
            Ok(snapshot) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::to_value(&snapshot).unwrap_or(Value::Null)),
                error: None,
            },
            Err(err) => routing_status_error_response(response_id, &identity, &err),
        };
    }
    match model_routing_status_for_member(
        &runtime.mob_handle(),
        runtime.mob_runtime().session_service(),
        &identity,
    )
    .await
    {
        Ok(snapshot) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(serde_json::to_value(&snapshot).unwrap_or(Value::Null)),
            error: None,
        },
        Err(err) => routing_status_error_response(response_id, &identity, &err),
    }
}

pub(super) async fn handle_force_cancel_member(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let member_id = params.get("member_id").and_then(Value::as_str);
    match member_id {
        Some(mid) if !mid.is_empty() => {
            let mid = crate::member_comms_id::runtime_alias_str(mid).into_owned();
            if let Some(response) =
                stale_runtime_alias_error_response(identity_runtime, &mid, response_id.clone())
                    .await
            {
                return response;
            }
            // The meerkat verb's typed `MobError` is carried out of the
            // identity operation intact, as `meta/stop_member_run` carries
            // it: a member that needs a runtime reload, an unknown member and
            // every other mob refusal keep meerkat's typed code and
            // structured data.
            let result: Result<Result<(), meerkat_mob::MobError>, String> = if let Some(identity_rt) =
                identity_runtime
                && let Some(identity) = identity_rt.identity_for_member_mutation(&mid).await
            {
                let handle = runtime.mob_handle();
                // Decode before keying: an `rt:` runtime alias names its durable
                // identity's roster row, exactly as the console path resolves it.
                let member_id = crate::member_comms_id::roster_member_id_for_supplied_id(&mid);
                identity_rt
                    .run_member_alias_operation_tracked(&identity, &mid, move || async move {
                        Ok(handle.force_cancel_member(member_id).await)
                    })
                    .await
                    .map_err(|error| error.to_string())
            } else if crate::member_comms_id::is_reserved_generated_alias(&mid) {
                Err(format!(
                    "generated member alias requires current identity authority: {mid}"
                ))
            } else {
                Ok(runtime
                    .mob_handle()
                    .force_cancel_member(crate::member_comms_id::mob_member_id(&mid))
                    .await)
            };
            match result {
                Ok(Ok(())) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({"accepted": true})),
                    error: None,
                },
                Ok(Err(mob_error)) => mob_declaration_error(response_id, &mob_error),
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("force_cancel_member failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: member_id required".to_string(),
                data: None,
            }),
        },
    }
}

fn stop_member_run_invalid_params(response_id: Value, message: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message: format!("Invalid params: {message}"),
            data: None,
        }),
    }
}

/// `meta/stop_member_run`: the run-fenced Stop of one exact member run
/// (`MobHandle::stop_member_run`). The member's runtime stops `run_id` and
/// terminalizes every input already bound to it, including durable steers
/// that joined it, and the reply carries meerkat's typed receipt verbatim. A
/// stale `run_id` is the `not_current` receipt, never an error.
pub(super) async fn handle_stop_member_run(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let Some(mid) = params
        .get("member_id")
        .and_then(Value::as_str)
        .filter(|mid| !mid.is_empty())
    else {
        return stop_member_run_invalid_params(response_id, "member_id required");
    };
    // Resolve the member before validating the rest, like every sibling
    // member verb: a stale runtime alias is reported as such.
    let mid = crate::member_comms_id::runtime_alias_str(mid).into_owned();
    if let Some(response) =
        stale_runtime_alias_error_response(identity_runtime, &mid, response_id.clone()).await
    {
        return response;
    }
    let Some(raw_run_id) = params.get("run_id").and_then(Value::as_str) else {
        return stop_member_run_invalid_params(response_id, "run_id required");
    };
    let Some(reason) = params
        .get("reason")
        .and_then(Value::as_str)
        .filter(|reason| !reason.is_empty())
    else {
        return stop_member_run_invalid_params(response_id, "reason required");
    };
    let run_id = match uuid::Uuid::parse_str(raw_run_id) {
        Ok(uuid) => meerkat_core::lifecycle::RunId::from_uuid(uuid),
        Err(error) => {
            return stop_member_run_invalid_params(
                response_id,
                &format!("run_id is not a UUID: {error}"),
            );
        }
    };
    let reason = reason.to_string();
    // The meerkat verb's typed `MobError` is carried out of the identity
    // operation intact, so unknown members and host refusals keep meerkat's
    // typed code and structured data.
    let result: Result<
        Result<meerkat_contracts::WireRunStopReceipt, meerkat_mob::MobError>,
        String,
    > = if let Some(identity_rt) = identity_runtime
        && let Some(identity) = identity_rt.identity_for_member_mutation(&mid).await
    {
        let handle = runtime.mob_handle();
        // Decode before keying: an `rt:` runtime alias names its durable
        // identity's roster row, exactly as the console path resolves it.
        let member_id = crate::member_comms_id::roster_member_id_for_supplied_id(&mid);
        identity_rt
            .run_member_alias_operation_tracked(&identity, &mid, move || async move {
                Ok(handle
                    .stop_member_run(
                        meerkat_mob::MobControlPrincipal::Owner,
                        member_id,
                        run_id,
                        reason,
                    )
                    .await)
            })
            .await
            .map_err(|error| error.to_string())
    } else if crate::member_comms_id::is_reserved_generated_alias(&mid) {
        Err(format!(
            "generated member alias requires current identity authority: {mid}"
        ))
    } else {
        Ok(runtime
            .mob_handle()
            .stop_member_run(
                meerkat_mob::MobControlPrincipal::Owner,
                crate::member_comms_id::mob_member_id(&mid),
                run_id,
                reason,
            )
            .await)
    };
    match result {
        Ok(Ok(receipt)) => match serde_json::to_value(&receipt) {
            Ok(receipt) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({"member_id": mid, "receipt": receipt})),
                error: None,
            },
            Err(error) => JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32603,
                    message: format!("failed to serialize run-stop receipt: {error}"),
                    data: None,
                }),
            },
        },
        Ok(Err(mob_error)) => mob_declaration_error(response_id, &mob_error),
        Err(err) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: format!("stop_member_run failed: {err}"),
                data: None,
            }),
        },
    }
}

/// Parse the bounded-result request fields required by meerkat 0.8.22's
/// exact helper contract. Presence is enforced here at the wire (-32602,
/// mirroring the meerkat-rpc contract's required fields); value validation
/// is owned upstream by `BoundedResultSpec::new` before admission.
fn parse_bounded_result_params(params: &Value) -> Result<(String, usize), String> {
    let result_label = params
        .get("result_label")
        .and_then(Value::as_str)
        .ok_or_else(|| "result_label required".to_string())?;
    let max_text_bytes = params
        .get("max_text_bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| "max_text_bytes required".to_string())?;
    let max_text_bytes = usize::try_from(max_text_bytes)
        .map_err(|_| "max_text_bytes exceeds platform bounds".to_string())?;
    Ok((result_label.to_string(), max_text_bytes))
}

pub(super) async fn handle_spawn_helper(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let agent_identity = params.get("agent_identity").and_then(Value::as_str);
    let task = params.get("task").and_then(Value::as_str);

    match (agent_identity, task) {
        (Some(mid), Some(task_str)) if !mid.is_empty() && !task_str.is_empty() => {
            let options = match parse_helper_options(params.get("options")) {
                Ok(opts) => opts,
                Err(msg) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {msg}"),
                            data: None,
                        }),
                    };
                }
            };
            let raw_reservation = match crate::member_comms_id::reserve_raw_member_target(
                identity_runtime,
                mid,
            )
            .await
            {
                Ok(reservation) => reservation,
                Err(message) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {message}"),
                            data: None,
                        }),
                    };
                }
            };
            let (result_label, max_text_bytes) = match parse_bounded_result_params(params) {
                Ok(bounded) => bounded,
                Err(msg) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {msg}"),
                            data: None,
                        }),
                    };
                }
            };
            let handle = runtime.mob_handle();
            let spawn_result = Box::pin(handle.spawn_helper(
                crate::member_comms_id::mob_member_id(raw_reservation.alias()),
                task_str,
                options,
                result_label,
                max_text_bytes,
            ))
            .await;
            drop(raw_reservation);
            match spawn_result {
                Ok(result) => {
                    // meerkat 0.8.22's bounded helper contract returns the
                    // exact turn carrier, so the session identity promised by
                    // the old comment is now real and re-added.
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "output": result.helper.output,
                            "tokens_used": result.helper.tokens_used,
                            "session_id": result.turn.result().session_id().to_string(),
                        })),
                        error: None,
                    }
                }
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("spawn_helper failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: agent_identity and task required".to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_fork_helper(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let source_member_id = params.get("source_member_id").and_then(Value::as_str);
    let agent_identity = params.get("agent_identity").and_then(Value::as_str);
    let task = params.get("task").and_then(Value::as_str);
    let fork_ctx_val = params.get("fork_context").cloned();

    match (source_member_id, agent_identity, task) {
        (Some(source), Some(mid), Some(task_str))
            if !source.is_empty() && !mid.is_empty() && !task_str.is_empty() =>
        {
            let source = crate::member_comms_id::runtime_alias_str(source).into_owned();
            if let Err(message) =
                crate::member_comms_id::validate_raw_member_target(identity_runtime, mid).await
            {
                return JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("Invalid params: {message}"),
                        data: None,
                    }),
                };
            }
            let fork_context = match fork_ctx_val {
                Some(v) if !v.is_null() => match serde_json::from_value::<ForkContext>(v) {
                    Ok(fc) => fc,
                    Err(err) => {
                        return JsonRpcResponse {
                            jsonrpc: JSONRPC_VERSION.to_string(),
                            id: response_id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32602,
                                message: format!("Invalid params: fork_context: {err}"),
                                data: None,
                            }),
                        };
                    }
                },
                _ => ForkContext::default(),
            };
            let options = match parse_helper_options(params.get("options")) {
                Ok(opts) => opts,
                Err(msg) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {msg}"),
                            data: None,
                        }),
                    };
                }
            };
            let (result_label, max_text_bytes) = match parse_bounded_result_params(params) {
                Ok(bounded) => bounded,
                Err(msg) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {msg}"),
                            data: None,
                        }),
                    };
                }
            };
            let handle = runtime.mob_handle();
            let helper_alias = mid.to_string();
            let task = task_str.to_string();
            let identity_runtime_owned = identity_runtime.cloned();
            let authority_target = if let Some(identity_runtime) = identity_runtime {
                identity_runtime
                    .member_alias_lifecycle_target(&source)
                    .await
            } else {
                Ok(None)
            };
            // A source the identity authority owns resolves to its durable
            // roster row: the lifecycle target already checked that a
            // generated alias is the current incarnation, and the tracked
            // operation checks it again under the lock. A classic source keeps
            // its own roster spelling.
            let source_member_id = if matches!(authority_target, Ok(Some(_))) {
                crate::member_comms_id::roster_member_id_for_supplied_id(&source)
            } else {
                crate::member_comms_id::mob_member_id(&source)
            };
            let fork_result = match authority_target {
                Err(error) => Err(error.to_string()),
                Ok(Some(target)) => {
                    crate::identity_first::IdentityRuntime::run_member_alias_targets_operation_tracked(
                        vec![target],
                        move || async move {
                            // The lifecycle target is acquired by the tracked
                            // wrapper before this closure runs. Reserve the raw
                            // namespace second, matching durable materialization's
                            // lock order and preventing an inversion deadlock.
                            let raw_reservation =
                                crate::member_comms_id::reserve_raw_member_target(
                                    identity_runtime_owned.as_ref(),
                                    helper_alias.as_str(),
                                )
                                .await?;
                            let helper_member_id = crate::member_comms_id::mob_member_id(
                                raw_reservation.alias(),
                            );
                            let result = handle
                            .fork_helper(
                                &source_member_id,
                                helper_member_id,
                                task.as_str(),
                                fork_context,
                                options,
                                result_label,
                                max_text_bytes,
                            )
                            .await
                            .map_err(|error| error.to_string());
                            drop(raw_reservation);
                            result
                        },
                    )
                    .await
                    .map_err(|error| error.to_string())
                }
                Ok(None) if crate::member_comms_id::is_reserved_generated_alias(&source) => Err(
                    format!("generated source alias requires current identity authority: {source}"),
                ),
                Ok(None) => {
                    match crate::member_comms_id::reserve_raw_member_target(
                        identity_runtime_owned.as_ref(),
                        helper_alias.as_str(),
                    )
                    .await
                    {
                        Err(error) => Err(error),
                        Ok(raw_reservation) => {
                            let helper_member_id =
                                crate::member_comms_id::mob_member_id(raw_reservation.alias());
                            let result = handle
                                .fork_helper(
                                    &source_member_id,
                                    helper_member_id,
                                    task.as_str(),
                                    fork_context,
                                    options,
                                    result_label,
                                    max_text_bytes,
                                )
                                .await
                                .map_err(|error| error.to_string());
                            drop(raw_reservation);
                            result
                        }
                    }
                }
            };
            match fork_result {
                Ok(result) => {
                    // See `handle_spawn_helper`: the bounded contract's exact
                    // turn carrier makes the session identity real, so it is
                    // re-added per the old comment's promise.
                    JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::json!({
                            "output": result.helper.output,
                            "tokens_used": result.helper.tokens_used,
                            "session_id": result.turn.result().session_id().to_string(),
                        })),
                        error: None,
                    }
                }
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("fork_helper failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: source_member_id, agent_identity, and task required"
                    .to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_attach_existing_session(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let role = params.get("role").and_then(Value::as_str);
    let agent_identity = params.get("agent_identity").and_then(Value::as_str);
    let session_id = params.get("session_id").and_then(Value::as_str);

    match (role, agent_identity, session_id) {
        (Some(role), Some(mid), Some(sid))
            if !role.is_empty() && !mid.is_empty() && !sid.is_empty() =>
        {
            let bridge_session_id = match meerkat_core::types::SessionId::parse(sid) {
                Ok(s) => s,
                Err(_) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: "Invalid params: session_id must be a valid session ID"
                                .to_string(),
                            data: None,
                        }),
                    };
                }
            };
            let raw_reservation = match crate::member_comms_id::reserve_raw_member_target(
                identity_runtime,
                mid,
            )
            .await
            {
                Ok(reservation) => reservation,
                Err(message) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {message}"),
                            data: None,
                        }),
                    };
                }
            };
            let identity = crate::member_comms_id::mob_member_id(raw_reservation.alias());
            let spec = SpawnMemberSpec::new(ProfileName::from(role), identity.clone())
                .with_launch_mode(MemberLaunchMode::Resume {
                    // 0.8.25: no migration authority on this path.
                    resume_from_role: None,
                    bridge_session_id,
                });
            let handle = runtime.mob_handle();
            let spawn_result = Box::pin(handle.spawn_spec(spec)).await;
            drop(raw_reservation);
            match spawn_result {
                Ok(_) => match handle.member_status(&identity).await {
                    Ok(snapshot) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: Some(serde_json::to_value(&snapshot).unwrap_or(Value::Null)),
                        error: None,
                    },
                    Err(err) => JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32000,
                            message: format!("attach_existing_session status lookup failed: {err}"),
                            data: None,
                        }),
                    },
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("attach_existing_session failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: role, agent_identity, and session_id required"
                    .to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_list_flows(
    runtime: &UnifiedRuntime,
    response_id: Value,
) -> JsonRpcResponse {
    let flows: Vec<String> = runtime
        .mob_handle()
        .list_flows()
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: Some(serde_json::json!({ "flows": flows })),
        error: None,
    }
}

/// Handle `meta/list_runs` — return [`meerkat_mob::run::MobRun`]
/// records for this mob, optionally filtered by `flow_id`. Each run
/// serializes via meerkat's existing `Serialize` impl, so the response
/// carries the full ledger projection (`step_ledger`, `failure_ledger`,
/// `frames`, `loops`, `loop_iteration_ledger`, `flow_state`, etc.).
pub(super) async fn handle_list_runs(
    runtime: &UnifiedRuntime,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let flow_id = params
        .get("flow_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(meerkat_mob::FlowId::from);
    match runtime.mob_handle().list_runs(flow_id.as_ref()).await {
        Ok(runs) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(serde_json::json!({
                "runs": serde_json::to_value(&runs).unwrap_or(Value::Null),
            })),
            error: None,
        },
        Err(err) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: format!("list_runs failed: {err}"),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_run_flow(
    runtime: &UnifiedRuntime,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let flow_id_str = params.get("flow_id").and_then(Value::as_str);
    let flow_params = params.get("params").cloned().unwrap_or(Value::Null);

    match flow_id_str {
        Some(fid) if !fid.is_empty() => {
            let flow_id = meerkat_mob::FlowId::from(fid);
            // An optional caller key admits the run once: a replay with the
            // same params and binding answers with the same run.
            let identity = match crate::flow_role_targets::run_flow_delivery_identity(params) {
                Ok(identity) => identity,
                Err(message) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {message}"),
                            data: None,
                        }),
                    };
                }
            };
            // `role_targets` binds each role of the flow to existing members;
            // meerkat's mob actor admits the binding and refuses it typed.
            // Each member id resolves by the member-id alias rule first, so a
            // stale generated alias is refused here, typed.
            let role_targets = match crate::flow_role_targets::resolve_role_targets(
                params,
                identity_runtime,
            )
            .await
            {
                Ok(role_targets) => role_targets,
                Err(crate::flow_role_targets::RoleTargetsRefusal::Invalid(message)) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: format!("Invalid params: {message}"),
                            data: None,
                        }),
                    };
                }
                Err(crate::flow_role_targets::RoleTargetsRefusal::StaleAlias(error)) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(error),
                    };
                }
            };
            let echo = role_targets
                .as_ref()
                .map(crate::flow_role_targets::role_targets_echo);
            let started = crate::flow_role_targets::start_run_flow(
                &runtime.mob_handle(),
                flow_id,
                flow_params,
                role_targets,
                identity,
            )
            .await;
            match started {
                Ok(run_id) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(crate::flow_role_targets::run_flow_result(&run_id, echo)),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("run_flow failed: {err}"),
                        data: crate::flow_role_targets::run_flow_error_data(&err),
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: flow_id required".to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_cancel_flow(
    runtime: &UnifiedRuntime,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let run_id = params.get("run_id").and_then(Value::as_str);
    match run_id {
        Some(rid) if !rid.is_empty() => {
            let run_id: meerkat_mob::RunId = match rid.parse() {
                Ok(id) => id,
                Err(_) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: "Invalid params: run_id not a valid run id".to_string(),
                            data: None,
                        }),
                    };
                }
            };
            match runtime.mob_handle().cancel_flow(run_id).await {
                Ok(()) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({"accepted": true})),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("cancel_flow failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: run_id required".to_string(),
                data: None,
            }),
        },
    }
}

pub(super) async fn handle_flow_status(
    runtime: &UnifiedRuntime,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let run_id = params.get("run_id").and_then(Value::as_str);
    match run_id {
        Some(rid) if !rid.is_empty() => {
            let run_id: meerkat_mob::RunId = match rid.parse() {
                Ok(id) => id,
                Err(_) => {
                    return JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        id: response_id,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: "Invalid params: run_id not a valid run id".to_string(),
                            data: None,
                        }),
                    };
                }
            };
            match runtime.mob_handle().flow_status(run_id).await {
                Ok(Some(mob_run)) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::to_value(&mob_run).unwrap_or(Value::Null)),
                    error: None,
                },
                Ok(None) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(Value::Null),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("flow_status failed: {err}"),
                        data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: run_id required".to_string(),
                data: None,
            }),
        },
    }
}

/// Wait until all current mob members are startup-ready for orchestration.
///
/// Relays meerkat 0.6's `MobHandle::wait_for_ready`. Returns a map of
/// `{ ready: [{ agent_identity, snapshot }], timeout: bool }`. The
/// `timeout` flag distinguishes "all members converged" from "partial
/// readiness, hit the wall".
pub(super) async fn handle_wait_ready(
    runtime: &UnifiedRuntime,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    // Omit `timeout_ms` => meta's generous default ceiling (the SDK contract
    // is "wait until ready"), not meerkat-mob 0.7.9's lowered 60s internal
    // default that `None` would otherwise inherit.
    let timeout = Some(
        params
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .map(std::time::Duration::from_millis)
            .unwrap_or(crate::unified_runtime::mob_ops::DEFAULT_WAIT_READY_TIMEOUT),
    );
    match runtime.mob_handle().wait_for_ready(timeout).await {
        Ok(ready) => {
            let entries: Vec<Value> = ready
                .into_iter()
                .map(|(identity, snapshot)| {
                    serde_json::json!({
                        "agent_identity": identity.to_string(),
                        "snapshot": serde_json::to_value(&snapshot).unwrap_or(Value::Null),
                    })
                })
                .collect();
            JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: Some(serde_json::json!({
                    "ready": entries,
                    "timeout": false,
                })),
                error: None,
            }
        }
        Err(err) => {
            if crate::unified_runtime::mob_ops::is_ready_wait_timeout(&err) {
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "ready": Vec::<Value>::new(),
                        "timeout": true,
                    })),
                    error: None,
                }
            } else {
                JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("wait_for_ready failed: {err}"),
                        data: None,
                    }),
                }
            }
        }
    }
}

pub(super) async fn handle_collect_completed(
    runtime: &UnifiedRuntime,
    response_id: Value,
) -> JsonRpcResponse {
    let completed = runtime.mob_handle().collect_completed().await;
    let entries: Vec<Value> = completed
        .into_iter()
        .map(|(mid, snapshot)| {
            serde_json::json!({
                "member_id": mid.to_string(),
                "snapshot": serde_json::to_value(&snapshot).unwrap_or(Value::Null),
            })
        })
        .collect();
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: Some(serde_json::json!({ "completed": entries })),
        error: None,
    }
}

/// Unwire a local member from a previously wired peer (local side only).
/// Symmetric counterpart to `handle_cross_mob_wire_local`.
pub(super) async fn handle_cross_mob_unwire_local(
    runtime: &UnifiedRuntime,
    runtime_owner: Option<std::sync::Arc<UnifiedRuntime>>,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let local_member_id = params.get("local_member_id").and_then(Value::as_str);
    let remote_comms_name = params.get("remote_comms_name").and_then(Value::as_str);
    let remote_peer_id = params.get("remote_peer_id").and_then(Value::as_str);
    let remote_address = params.get("remote_address").and_then(Value::as_str);
    let remote_pubkey = match parse_optional_pubkey(params, "remote_pubkey_b64") {
        Ok(value) => value,
        Err(err) => {
            return JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {err}"),
                    data: None,
                }),
            };
        }
    };

    match (local_member_id, remote_comms_name, remote_peer_id, remote_address) {
        (Some(local), Some(comms_name), Some(peer_id), Some(addr))
            if !local.is_empty()
                && !comms_name.is_empty()
                && !peer_id.is_empty()
                && !addr.is_empty() =>
        {
            let owned_local = local.to_string();
            let owned_comms_name = comms_name.to_string();
            let owned_peer_id = peer_id.to_string();
            let owned_addr = addr.to_string();
            let owned_identity_runtime = identity_runtime.cloned();
            let borrowed_identity_runtime = identity_runtime.cloned();
            match run_cross_mob_local_member_operation(
                runtime,
                runtime_owner,
                identity_runtime,
                local,
                move |runtime| async move {
                    runtime
                        .unwire_local_with_identity_runtime(
                            &owned_local,
                            &owned_comms_name,
                            &owned_peer_id,
                            &owned_addr,
                            remote_pubkey,
                            owned_identity_runtime.as_ref(),
                        )
                        .await
                        .map_err(|err| err.to_string())
                },
                || async move {
                    runtime
                        .unwire_local_with_identity_runtime(
                            local,
                            comms_name,
                            peer_id,
                            addr,
                            remote_pubkey,
                            borrowed_identity_runtime.as_ref(),
                        )
                        .await
                        .map_err(|err| err.to_string())
                },
            )
            .await
            {
                Ok(()) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "accepted": true,
                        "local_member_id": local,
                        "remote_comms_name": comms_name,
                    })),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("cross_mob/unwire_local failed: {err}"),
                    data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: local_member_id, remote_comms_name, remote_peer_id, and remote_address required".to_string(),
                    data: None,
                }),
        },
    }
}

/// Wire a local member to an external peer using a provided spec.
/// Only wires the local side — the remote side must do its own call.
pub(super) async fn handle_cross_mob_wire_local(
    runtime: &UnifiedRuntime,
    runtime_owner: Option<std::sync::Arc<UnifiedRuntime>>,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let identity_runtime = identity_runtime.or_else(|| runtime.identity_runtime_ref());
    let local_member_id = params.get("local_member_id").and_then(Value::as_str);
    let remote_comms_name = params.get("remote_comms_name").and_then(Value::as_str);
    let remote_peer_id = params.get("remote_peer_id").and_then(Value::as_str);
    let remote_address = params.get("remote_address").and_then(Value::as_str);
    let remote_pubkey = match parse_optional_pubkey(params, "remote_pubkey_b64") {
        Ok(value) => value,
        Err(err) => {
            return JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32602,
                    message: format!("Invalid params: {err}"),
                    data: None,
                }),
            };
        }
    };

    match (local_member_id, remote_comms_name, remote_peer_id, remote_address) {
        (Some(local), Some(comms_name), Some(peer_id), Some(addr))
            if !local.is_empty()
                && !comms_name.is_empty()
                && !peer_id.is_empty()
                && !addr.is_empty() =>
        {
            let owned_local = local.to_string();
            let owned_comms_name = comms_name.to_string();
            let owned_peer_id = peer_id.to_string();
            let owned_addr = addr.to_string();
            let owned_identity_runtime = identity_runtime.cloned();
            let borrowed_identity_runtime = identity_runtime.cloned();
            match run_cross_mob_local_member_operation(
                runtime,
                runtime_owner,
                identity_runtime,
                local,
                move |runtime| async move {
                    runtime
                        .wire_local_with_identity_runtime(
                            &owned_local,
                            &owned_comms_name,
                            &owned_peer_id,
                            &owned_addr,
                            remote_pubkey,
                            owned_identity_runtime.as_ref(),
                        )
                        .await
                        .map_err(|err| err.to_string())
                },
                || async move {
                    runtime
                        .wire_local_with_identity_runtime(
                            local,
                            comms_name,
                            peer_id,
                            addr,
                            remote_pubkey,
                            borrowed_identity_runtime.as_ref(),
                        )
                        .await
                        .map_err(|err| err.to_string())
                },
            )
            .await
            {
                Ok(()) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "accepted": true,
                        "local_member_id": local,
                        "remote_comms_name": comms_name,
                    })),
                    error: None,
                },
                Err(err) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("cross_mob/wire_local failed: {err}"),
                    data: None,
                    }),
                },
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: local_member_id, remote_comms_name, remote_peer_id, and remote_address required".to_string(),
                    data: None,
                }),
        },
    }
}

// ---------------------------------------------------------------------------
// Mob/run labels — meta-side sidecar metadata
// ---------------------------------------------------------------------------

use crate::runtime::{LabelRpcResult, dispatch_label_method, labels_to_json_value};

fn label_response(response_id: Value, outcome: LabelRpcResult) -> JsonRpcResponse {
    let (result, error) = match outcome {
        LabelRpcResult::Accepted => (Some(serde_json::json!({"accepted": true})), None),
        LabelRpcResult::Labels(labels) => (
            Some(serde_json::json!({"labels": labels_to_json_value(&labels)})),
            None,
        ),
        LabelRpcResult::InvalidParams(message) => (
            None,
            Some(JsonRpcError {
                code: -32602,
                message: format!("Invalid params: {message}"),
                data: None,
            }),
        ),
    };
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result,
        error,
    }
}

pub(super) async fn handle_label_method(
    runtime: &UnifiedRuntime,
    method: &str,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    match dispatch_label_method(runtime.metadata_table(), &runtime.mob_id(), method, params).await {
        Some(outcome) => label_response(response_id, outcome),
        None => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            }),
        },
    }
}

// ---------------------------------------------------------------------------
// Cross-mob peer trust — Ed25519 peer descriptor support
// ---------------------------------------------------------------------------

/// Parse an optional `<field>` (base64 Ed25519 pubkey) from a params object.
///
/// Returns `Ok(None)` if the field is absent or null. Returns the decoded
/// 32 bytes if the field is a non-empty string. Empty strings are treated
/// as absent so callers can pass `""` from a UI to mean "use defaults".
fn parse_optional_pubkey(params: &Value, field: &str) -> Result<Option<[u8; 32]>, String> {
    let value = match params.get(field) {
        Some(v) => v,
        None => return Ok(None),
    };
    if value.is_null() {
        return Ok(None);
    }
    let s = value
        .as_str()
        .ok_or_else(|| format!("{field} must be a base64 string"))?;
    if s.is_empty() {
        return Ok(None);
    }
    // Peer descriptors emit `transport_public_key` with an `ed25519:` scheme
    // prefix; callers round-tripping that value into `wire_local` had to
    // strip it by hand (downstream app DX report, 2026-07-09). Accept both
    // spellings.
    let s = s.strip_prefix("ed25519:").unwrap_or(s);
    crate::auth::peer_keys::decode_pubkey_b64(s)
        .map(Some)
        .map_err(|err| format!("{field}: {err}"))
}

/// Handle `meta/peer_pubkey` — return the local gateway's Ed25519
/// signing pubkey so peer gateways can bootstrap trust.
///
/// Returns `{"pubkey_b64": "<base64>"}` when a keypair is configured.
/// Returns a `-32004 capability_unavailable` error when the gateway has
/// no keys (e.g. inproc-only deployment that never called
/// `set_gateway_peer_keys`) so callers can fall back to TOFU rather than
/// stalling.
pub(super) async fn handle_peer_pubkey(
    runtime: &UnifiedRuntime,
    response_id: Value,
) -> JsonRpcResponse {
    match runtime.gateway_peer_keys() {
        Some(keys) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: Some(serde_json::json!({
                "pubkey_b64": keys.pubkey_b64(),
            })),
            error: None,
        },
        None => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32004,
                message: "gateway has no signing keypair configured".to_string(),
                data: None,
            }),
        },
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod member_declaration_error_parity_tests {
    //! The PR claims these methods classify a failure the same way rkat-rpc does.
    //! That claim needs a test: meerkat's mapping lives behind a trait private to
    //! its handler module, so this one is reproduced from `MobError`'s public
    //! accessors and could silently diverge. Asserting the CODE and the typed
    //! DATA is what makes the parity claim falsifiable rather than aspirational.

    use meerkat_contracts::ErrorCode;

    use super::mob_declaration_error;

    fn error_for(error: &meerkat_mob::MobError) -> (i64, Option<serde_json::Value>) {
        let response = mob_declaration_error(serde_json::json!("parity-test"), error);
        let rendered = response.error.expect("a MobError must render as an error");
        assert!(
            response.result.is_none(),
            "an error response must not also carry a result"
        );
        (rendered.code, rendered.data)
    }

    /// A classified error must carry meerkat's own jsonrpc code plus the typed
    /// recovery facts, not a generic -32602 with a prose message.
    #[test]
    fn a_classified_error_keeps_its_code_and_typed_data() {
        let (code, data) = error_for(&meerkat_mob::MobError::StaleEventCursor {
            after_cursor: 40,
            latest_cursor: 25,
        });
        assert_eq!(
            code,
            i64::from(ErrorCode::StaleCursor.jsonrpc_code()),
            "the classified code must survive, or a caller cannot branch on it"
        );
        let data = data.expect("stale-cursor data is the recovery fact");
        assert_eq!(data["watermark"], serde_json::json!(25));
        assert_eq!(data["requested"], serde_json::json!(40));
    }

    #[test]
    fn a_second_class_maps_to_its_own_code_rather_than_a_shared_one() {
        let (code, data) = error_for(&meerkat_mob::MobError::StaleFenceToken {
            runtime_id: meerkat_mob::AgentRuntimeId::initial(meerkat_mob::AgentIdentity::from(
                "worker",
            )),
            expected: meerkat_mob::FenceToken::new(3),
            actual: meerkat_mob::FenceToken::new(2),
        });
        assert_eq!(code, i64::from(ErrorCode::StaleFence.jsonrpc_code()));
        let data = data.expect("stale-fence data carries the fence numbers");
        assert_eq!(data["expected"], serde_json::json!(3));
        assert_eq!(data["actual"], serde_json::json!(2));
        // Distinctness is the point: if both classes collapsed to one code the
        // two assertions above would still pass individually.
        assert_ne!(
            ErrorCode::StaleFence.jsonrpc_code(),
            ErrorCode::StaleCursor.jsonrpc_code()
        );
    }

    /// A member that needs a runtime reload keeps meerkat's own code and the
    /// named repair, so a caller of `meta/force_cancel_member` can tell it
    /// apart from every other refusal and reload the member's registration.
    #[test]
    fn a_member_that_needs_a_reload_keeps_its_code_and_named_repair() {
        let (code, data) = error_for(&meerkat_mob::MobError::MemberReloadRequired {
            member_id: meerkat_mob::AgentIdentity::from("worker"),
            reason: "runtime registration is degraded".to_string(),
        });
        assert_eq!(
            code,
            i64::from(ErrorCode::MemberReloadRequired.jsonrpc_code()),
            "the reload-required code must survive, or a caller cannot branch on it"
        );
        let data = data.expect("reload-required data names the member and the repair");
        assert_eq!(
            data["kind"],
            serde_json::json!("mob_member_reload_required")
        );
        assert_eq!(data["member_id"], serde_json::json!("worker"));
        assert_eq!(
            data["reason"],
            serde_json::json!("runtime registration is degraded")
        );
        assert_eq!(
            data["required_action"],
            serde_json::json!("reload_member_registration")
        );
    }

    /// An UNCLASSIFIED error must still be an error, and must fall back to
    /// invalid-params rather than inventing a classified code it does not have.
    #[test]
    fn an_unclassified_error_falls_back_without_fabricating_a_class() {
        let (code, _) = error_for(&meerkat_mob::MobError::Internal(
            "no wire classification for this one".to_string(),
        ));
        assert_eq!(
            code, -32602,
            "an unclassified MobError must not borrow a classified code"
        );
    }

    /// A durable profile on a mob whose session service keeps no persistent
    /// sessions keeps meerkat's closed refusal: its capability code and the
    /// member and profile it names.
    #[test]
    fn a_durable_profile_refusal_keeps_its_code_and_typed_data() {
        let (code, data) = error_for(&meerkat_mob::MobError::NonDurableService {
            member_id: meerkat_mob::AgentIdentity::from("worker"),
            profile: meerkat_mob::ProfileName::from("durable-worker"),
        });
        assert_eq!(
            code,
            i64::from(ErrorCode::CapabilityUnavailable.jsonrpc_code())
        );
        let data = data.expect("the refusal names the member and the profile");
        assert_eq!(data["kind"], serde_json::json!("mob_non_durable_service"));
        assert_eq!(data["member_id"], serde_json::json!("worker"));
        assert_eq!(data["profile"], serde_json::json!("durable-worker"));
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod durable_profile_refusal_tests {
    //! A profile that declares `durable = true` on a mob whose session
    //! service keeps no persistent sessions: the worker-plane ensure answers
    //! meerkat's typed refusal, not the generic invalid-params text.

    use std::sync::Arc;

    use meerkat_contracts::ErrorCode;

    #[tokio::test]
    async fn ensure_member_answers_meerkats_durable_profile_refusal() {
        // A mob member profile needs comms on: Core refuses one without it
        // as a wiring error. With comms on, the profile's only refusal here
        // is `durable` on a mob that keeps no persistent sessions.
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "durable-profile-refusal"

[profiles.worker]
model = "gpt-5.5"
durable = true

[profiles.worker.tools]
comms = true
"#,
        )
        .expect("definition parses");
        // An ephemeral session service keeps no persistent sessions, so its
        // member sessions do not outlive the process. The builder's own
        // composition refuses a durable profile as well, through
        // `keeps_durable_member_sessions` (durable_profile_composition_tests).
        let sessions = tempfile::tempdir().expect("session dir");
        let service = Arc::new(meerkat::build_ephemeral_service(
            meerkat::AgentFactory::new(sessions.path()).comms(true),
            meerkat::Config::default(),
            4,
        ));
        let spec =
            crate::MobBootstrapSpec::new(definition, meerkat_mob::MobStorage::in_memory(), service)
                .with_options(crate::MobBootstrapOptions {
                    allow_ephemeral_sessions: true,
                    notify_orchestrator_on_resume: true,
                    default_llm_client: Some(Arc::new(meerkat_client::TestClient::for_provider(
                        meerkat_core::Provider::OpenAI,
                    ))),
                });
        let runtime = crate::unified_runtime::UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(crate::MetaConfig {
                modules: Vec::new(),
                discovery: crate::DiscoverySpec {
                    namespace: "durable-profile-refusal".to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            })
            .timeout(std::time::Duration::from_secs(1))
            .build()
            .await
            .expect("runtime builds");
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "durable-profile",
            "method": "meta/ensure_member",
            "params": { "role": "worker", "agent_identity": "worker-1" },
        })
        .to_string();
        let raw = crate::rpc::handle_unified_rpc_json(
            &runtime,
            &request,
            std::time::Duration::from_secs(5),
            None,
            None,
        )
        .await;
        let response: serde_json::Value =
            serde_json::from_str(&raw).expect("router must answer with JSON");
        let error = response
            .get("error")
            .expect("the spawn of a durable profile is refused");
        assert_eq!(
            error["code"],
            serde_json::json!(ErrorCode::CapabilityUnavailable.jsonrpc_code()),
            "{response}"
        );
        assert_eq!(
            error["data"]["kind"],
            serde_json::json!("mob_non_durable_service"),
            "{response}"
        );
        assert_eq!(error["data"]["profile"], serde_json::json!("worker"));
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod durable_profile_composition_tests {
    //! Whether Meta's compositions admit a durable worker profile. The
    //! default composition (no `persistent_state`, no storage provider) runs
    //! Meerkat's persistent session service over in-memory stores: it keeps
    //! the persistent-session contract, but its member sessions do not
    //! outlive the process, so a durable profile is refused there with
    //! Meerkat's typed refusal and remedy. With `persistent_state` the member
    //! sessions live in the state directory and the profile is admitted.

    use std::sync::Arc;

    use meerkat_contracts::ErrorCode;

    const DURABLE_WORKER_TOML: &str = r#"
[mob]
id = "MOB_ID"

[profiles.worker]
model = "gpt-5.5"
durable = true

[profiles.worker.tools]
comms = true
"#;

    /// The same worker, bound to the realm profile `durable-worker`.
    const REALM_WORKER_TOML: &str = r#"
[mob]
id = "MOB_ID"

[profiles.worker]
realm_profile = "durable-worker"
"#;

    /// `toml` for the mob `mob_id`: each row runs its own mob, since the
    /// rows share one process and a mob's supervisor name is process-wide.
    fn definition(toml: &str, mob_id: &str) -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml(&toml.replace("MOB_ID", mob_id))
            .expect("definition parses")
    }

    fn test_client() -> Arc<meerkat_client::TestClient> {
        Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        ))
    }

    /// `meta/ensure_member` of a `worker` on `runtime`, as its JSON answer.
    async fn ensure_worker(runtime: &crate::unified_runtime::UnifiedRuntime) -> serde_json::Value {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "durable-profile",
            "method": "meta/ensure_member",
            "params": { "role": "worker", "agent_identity": "worker-1" },
        })
        .to_string();
        let raw = crate::rpc::handle_unified_rpc_json(
            runtime,
            &request,
            std::time::Duration::from_secs(5),
            None,
            None,
        )
        .await;
        serde_json::from_str(&raw).expect("router must answer with JSON")
    }

    /// Meerkat's durable-profile refusal, with its remedy.
    fn assert_refused_with_the_remedy(response: &serde_json::Value, shape: &str) {
        let error = response.get("error").unwrap_or_else(|| {
            panic!("{shape}: the spawn of a durable profile is refused: {response}")
        });
        assert_eq!(
            error["code"],
            serde_json::json!(ErrorCode::CapabilityUnavailable.jsonrpc_code()),
            "{shape}: {response}"
        );
        assert_eq!(
            error["data"]["kind"],
            serde_json::json!("mob_non_durable_service"),
            "{shape}: {response}"
        );
        assert_eq!(
            error["data"]["profile"],
            serde_json::json!("worker"),
            "{shape}"
        );
        let message = error["message"].as_str().unwrap_or_default();
        assert!(
            message
                .contains("run the mob on a persistent realm, or drop `durable` from the profile"),
            "{shape}: the refusal names its remedy: {response}"
        );
    }

    #[tokio::test]
    async fn the_default_composition_refuses_an_inline_durable_profile() {
        let runtime = crate::unified_runtime::UnifiedRuntime::builder()
            .definition(definition(DURABLE_WORKER_TOML, "durable-profile-inline"))
            .default_llm_client(test_client())
            .build()
            .await
            .expect("runtime builds");
        assert_refused_with_the_remedy(&ensure_worker(&runtime).await, "inline profile");
    }

    /// A realm profile resolves at spawn, so Meerkat's own spawn check is
    /// what refuses it.
    #[tokio::test]
    async fn the_default_composition_refuses_a_durable_realm_profile() {
        let realm_profiles = Arc::new(meerkat_mob::InMemoryRealmProfileStore::new());
        let durable = definition(DURABLE_WORKER_TOML, "durable-profile-realm").profiles["worker"]
            .as_inline()
            .cloned()
            .expect("the inline durable worker");
        meerkat_mob::RealmProfileStore::create(&*realm_profiles, "durable-worker", &durable)
            .await
            .expect("store the durable realm profile");
        let runtime = crate::unified_runtime::UnifiedRuntime::builder()
            .definition(definition(REALM_WORKER_TOML, "durable-profile-realm"))
            .mob_storage(
                meerkat_mob::MobStorage::in_memory().with_realm_profile_store(Some(
                    realm_profiles as Arc<dyn meerkat_mob::RealmProfileStore>,
                )),
                crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            )
            .default_llm_client(test_client())
            .build()
            .await
            .expect("runtime builds");
        assert_refused_with_the_remedy(&ensure_worker(&runtime).await, "realm profile");
    }

    #[tokio::test]
    async fn persistent_state_admits_a_durable_profile() {
        let state = tempfile::tempdir().expect("state dir");
        let runtime = crate::unified_runtime::UnifiedRuntime::builder()
            .definition(definition(
                DURABLE_WORKER_TOML,
                "durable-profile-persistent",
            ))
            .persistent_state(state.path())
            .default_llm_client(test_client())
            .build()
            .await
            .expect("runtime builds");
        let response = ensure_worker(&runtime).await;
        assert!(
            response.get("error").is_none() && response.get("result").is_some(),
            "a durable profile spawns where member sessions outlive the process: {response}"
        );
        runtime.shutdown().await;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod member_declaration_delegation_tests {
    //! End-to-end through the REAL router with a REAL runtime.
    //!
    //! The defect these close is not a wrong answer, it is `method not found`:
    //! meerkat shipped both handle operations and Meta reached neither, so a
    //! host could supply a compiled policy at init and never bind a member to it.
    //! The assertion that matters is therefore that these methods are ROUTED and
    //! land in a handler that talks to the mob handle.

    use crate::unified_runtime::lifecycle::stop_degrade_tests::empty_runtime;

    const METHOD_NOT_FOUND: i64 = -32601;

    async fn route(mob: &str, method: &str, params: serde_json::Value) -> serde_json::Value {
        let runtime = empty_runtime(mob).await;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "delegation-test",
            "method": method,
            "params": params,
        })
        .to_string();
        let raw = crate::rpc::handle_unified_rpc_json(
            &runtime,
            &request,
            std::time::Duration::from_secs(5),
            None,
            None,
        )
        .await;
        serde_json::from_str(&raw).expect("router must answer with JSON")
    }

    /// All THREE methods must be reachable. Before this change the router had no
    /// `mob/` namespace at all, so every one of these returned method-not-found.
    #[tokio::test]
    async fn all_three_declaration_methods_are_routed_not_method_not_found() {
        // A DISTINCT mob per iteration: the comms participant name registry is
        // process-global, so reusing one id makes the second bootstrap fail with
        // ParticipantNameOccupied and the test reports a routing problem it never
        // reached.
        for (index, method) in [
            "mob/adopt_member_identity_declaration",
            "mob/apply_member_tool_declaration",
            "mob/member_tool_declaration",
        ]
        .into_iter()
        .enumerate()
        {
            let response = route(
                &format!("declaration-routing-{index}"),
                method,
                serde_json::json!({}),
            )
            .await;
            let code = response
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(serde_json::Value::as_i64);
            assert_ne!(
                code,
                Some(METHOD_NOT_FOUND),
                "{method} is still unroutable, which is the entire defect"
            );
        }
    }

    /// Reaching the handler is not enough: it must reach the handle's mob. A
    /// payload aimed elsewhere is refused with a typed reason rather than being
    /// silently retargeted at this gateway's mob and reported as success.
    #[tokio::test]
    async fn a_payload_for_another_mob_is_refused_with_a_typed_reason() {
        let response = route(
            "declaration-own-mob",
            "mob/apply_member_tool_declaration",
            serde_json::json!({
                "mob_id": "some-other-mob",
                "agent_identity": "identity:member-2",
                "request_id": "ex-tp-apply-identity-member-2-rev109-0001",
                "expected_intent_revision": 3,
                "declaration": {
                    "category_overrides": {},
                    "callback_tools": { "kind": "inherit" },
                    "execution": { "kind": "inherit" },
                    "application_policy": {
                        "kind": "provider",
                        "provider_id": "example",
                        "policy_id": "example-tools"
                    }
                },
                "convergence": { "kind": "drain", "max_wait_ms": 120000 }
            }),
        )
        .await;
        let error = response
            .get("error")
            .expect("a foreign mob must be refused");
        assert_eq!(
            error.get("data").and_then(|data| data.get("kind")),
            Some(&serde_json::json!("foreign_mob_target")),
            "the refusal must say WHY, not just fail: {response}"
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod member_declaration_surface_tests {
    //! The four-surface serve-or-omit split.
    //!
    //! #343 registered TWO of four surfaces and 0.8.21 shipped the methods dead
    //! on the console plane. My first correction said "three sites", which was
    //! also wrong: there are FOUR dispatch/advertising surfaces and the correct
    //! answer DIFFERS per surface, because two of them cannot serve the family at
    //! all. A registry guarantees they cannot disagree about MEMBERSHIP; only an
    //! explicit per-surface decision covers serve-or-omit, and only a test pins
    //! that decision.

    /// Source of one function, bounded by the next top-level `fn`.
    ///
    /// Slicing to end-of-file would make every assertion below pass on any file
    /// that mentions the registry ANYWHERE - which is exactly how a whole-file
    /// grep would have called 0.8.21 correct.
    fn function_source(file: &str, needle: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let start = text
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} not found in {file}"));
        let rest = &text[start + needle.len()..];
        let end = rest
            .find("\nasync fn ")
            .into_iter()
            .chain(rest.find("\nfn "))
            .chain(rest.find("\npub fn "))
            .chain(rest.find("\npub async fn "))
            .min()
            .unwrap_or(rest.len());
        text[start..start + needle.len() + end].to_string()
    }

    const REGISTRY: &str = "MEMBER_DECLARATION_METHODS";

    #[test]
    fn the_two_serving_surfaces_advertise_from_the_registry() {
        for (file, needle) in [
            ("src/rpc.rs", "async fn handle_unified_rpc_json_inner("),
            (
                "src/http_console.rs",
                "async fn handle_console_runtime_rpc_with_visibility(",
            ),
        ] {
            let body = function_source(file, needle);
            assert!(
                body.contains(REGISTRY),
                "{needle} serves the family but does not advertise from the registry"
            );
        }
    }

    /// The half a registry cannot enforce. Both of these lack a `MobHandle`, so
    /// they cannot serve the family; advertising what a surface cannot serve is
    /// the same defect as serving what it does not advertise, and I shipped BOTH
    /// halves of that in one edit earlier by matching the wrong function.
    #[test]
    fn the_two_non_serving_surfaces_advertise_nothing_from_it() {
        for (file, needle, why) in [
            (
                "src/rpc.rs",
                "pub fn handle_meta_rpc_json(",
                "takes MetaRuntimeHandle: module runtime, no MobHandle",
            ),
            (
                "src/http_console.rs",
                "async fn handle_console_aggregator_rpc(",
                "aggregator-only: no single MobRuntime authority",
            ),
        ] {
            let body = function_source(file, needle);
            assert!(
                !body.contains(REGISTRY),
                "{needle} cannot serve the family ({why}) yet advertises it"
            );
        }
    }

    /// The registry is the single source, so a surface cannot hand-list a member.
    #[test]
    fn no_surface_hand_lists_a_family_method() {
        for file in ["src/rpc.rs", "src/http_console.rs"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
            let text = std::fs::read_to_string(&path).expect("read surface");
            for method in super::MEMBER_DECLARATION_METHODS {
                assert!(
                    !text.contains(&format!("\"{method}\"")),
                    "{file} hand-lists {method} as a literal instead of deriving it"
                );
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod member_declaration_alias_tests {
    //! The alias boundary, asserted in BOTH directions.
    //!
    //! 0.8.21 shipped these methods live and unable to name anything: the roster
    //! is keyed by the comms-safe encoded id, public surfaces speak aliases, and
    //! the handlers used the alias verbatim. Fixing only the inbound half then
    //! leaked the reserved encoded spelling outbound - which the public input
    //! validator REFUSES, so a caller could not feed our own response back to us.

    use super::*;

    /// Reserved-namespace rejection, asserted RECURSIVELY rather than field by
    /// field.
    ///
    /// A per-field assertion can only cover the fields I remembered, and
    /// forgetting one is precisely the defect: I fixed the read's own
    /// `agent_identity` and missed the `agent_identity` nested inside every
    /// `WireIdentityConvergenceStatus`. Walking the whole response means a field
    /// added later, or one I never knew about, still fails here.
    fn assert_no_reserved_namespace(value: &serde_json::Value, path: &str) {
        match value {
            serde_json::Value::String(text) => assert!(
                !text.contains("mk--"),
                "{path} leaks the reserved roster namespace: {text}"
            ),
            serde_json::Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    assert_no_reserved_namespace(item, &format!("{path}[{index}]"));
                }
            }
            serde_json::Value::Object(fields) => {
                for (key, item) in fields {
                    assert_no_reserved_namespace(item, &format!("{path}.{key}"));
                }
            }
            _ => {}
        }
    }

    /// The encoding is real: an identity-first alias does NOT survive as itself.
    /// If this ever passes trivially, the fixture has stopped exercising the case
    /// that broke, and the mirroring tests below would be vacuous.
    #[test]
    fn the_alias_encoding_is_actually_lossy_in_spelling() {
        let alias = "rt:gate:main:0";
        let roster = crate::member_comms_id::mob_member_id_str(alias);
        assert_ne!(
            roster.as_ref(),
            alias,
            "an identity-first alias must encode to a different roster key, or \
             these tests prove nothing"
        );
        assert!(roster.starts_with("mk--"), "roster key: {roster}");
    }

    /// Both directions, composed: alias in, alias out.
    #[test]
    fn an_alias_round_trips_through_the_boundary() {
        for alias in [
            "rt:gate:main:0",
            "identity:member-2",
            "gate:main",
            "plainname",
        ] {
            let roster = crate::member_comms_id::mob_member_id_str(alias);
            let back = public_member_alias(roster.as_ref());
            assert_eq!(back, alias, "alias {alias} did not survive the round trip");
        }
    }

    /// The exact leak that shipped: a convergence status carrying the roster key
    /// must be mirrored back to the alias before it goes out.
    #[test]
    fn a_convergence_status_is_mirrored_before_it_leaves() {
        let roster = crate::member_comms_id::mob_member_id_str("rt:gate:main:0").into_owned();
        let leaked = meerkat_contracts::wire::WireIdentityConvergenceStatus {
            agent_identity: roster,
            desired_intent_revision: Some(3),
            active_intent_revision: None,
            decision: None,
            condition: meerkat_contracts::wire::WireIdentityConvergenceCondition::Converged,
            observed_at_ms: 0,
            detail: None,
        };
        assert!(
            leaked.agent_identity.contains("mk--"),
            "fixture must start leaked"
        );

        let mirrored = mirror_convergence(leaked);
        assert_eq!(mirrored.agent_identity, "rt:gate:main:0");
        let json = serde_json::to_value(&mirrored).expect("serializes");
        assert_no_reserved_namespace(&json, "convergence");
    }

    /// Same assertion over the whole read result, recursively, because that is the
    /// shape that catches a field nobody enumerated.
    #[test]
    fn no_field_of_a_read_result_carries_the_reserved_namespace() {
        let roster = crate::member_comms_id::mob_member_id_str("identity:member-2").into_owned();
        let result = meerkat_contracts::wire::MobMemberToolDeclarationResult {
            mob_id: "example".to_string(),
            agent_identity: public_member_alias(&roster),
            desired_intent_revision: 3,
            // Real declaration shape rather than a synthetic default, so the
            // recursive walk covers the nested fields a caller actually receives.
            declaration: serde_json::from_value(serde_json::json!({
                "category_overrides": {},
                "callback_tools": { "kind": "inherit" },
                "execution": { "kind": "inherit" },
                "application_policy": {
                    "kind": "provider",
                    "provider_id": "example",
                    "policy_id": "example-tools"
                }
            }))
            .expect("declaration fixture must deserialize"),
            convergence: mirror_convergence(
                meerkat_contracts::wire::WireIdentityConvergenceStatus {
                    agent_identity: roster,
                    desired_intent_revision: Some(3),
                    active_intent_revision: None,
                    decision: None,
                    condition: meerkat_contracts::wire::WireIdentityConvergenceCondition::Converged,
                    observed_at_ms: 0,
                    detail: None,
                },
            ),
        };
        let json = serde_json::to_value(&result).expect("serializes");
        assert_no_reserved_namespace(&json, "read_result");
        assert_eq!(
            json["agent_identity"],
            serde_json::json!("identity:member-2")
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod member_declaration_wire_tests {
    //! Contract tests for the `mob/*` member-declaration surface.
    //!
    //! These assert against an exact caller-shaped payload (a Phase B apply as an
    //! external caller sends it, with synthetic values), rather than against a
    //! shape inferred from the struct definitions. That distinction is the
    //! point: the two previous gaps on this surface (#337's carrier and the
    //! role-migration carrier) were both "the type existed and the caller could
    //! not reach it", and a payload inferred from the types alone would
    //! reproduce the same class of mistake one level up.

    /// The apply request an external caller sends once per member per rollout.
    const DOWNSTREAM_APPLY_PARAMS: &str = r#"{
      "mob_id": "example",
      "agent_identity": "identity:member-2",
      "request_id": "ex-tp-apply-identity-member-2-rev109-0001",
      "expected_intent_revision": 3,
      "declaration": {
        "category_overrides": {
          "builtins": "inherit",
          "shell": "inherit",
          "comms": "inherit",
          "mob": "inherit",
          "memory": "inherit",
          "schedule": "inherit",
          "workgraph": "inherit",
          "image_generation": "inherit",
          "web_search": "inherit"
        },
        "callback_tools": { "kind": "inherit" },
        "execution": { "kind": "inherit" },
        "application_policy": {
          "kind": "provider",
          "provider_id": "example",
          "policy_id": "example-tools"
        }
      },
      "convergence": { "kind": "drain", "max_wait_ms": 120000 }
    }"#;

    /// The adopt request, sent once per member before its first apply, in the
    /// shape an external adopt runner sends; per-member variation is only
    /// agent_identity, request_id, the session values and profile_name.
    const DOWNSTREAM_ADOPT_PARAMS: &str = r#"{
      "mob_id": "example",
      "agent_identity": "identity:member-2",
      "request_id": "ex-tp-adopt-identity-member-2-0001",
      "precondition": "expected_absent",
      "declaration_scope": "example-tool-policy",
      "declaration_revision": 1,
      "session": {
        "session_id": "0190a000-0000-7000-8000-000000000001",
        "lineage_id": "session:0190a000-0000-7000-8000-000000000001",
        "lineage_generation": 0,
        "authority_policy": "require_existing"
      },
      "member": {
        "profile_name": "identity",
        "runtime_mode": "turn_driven",
        "execution": { "execution": "controlling_session" }
      },
      "owned_wiring": [],
      "convergence": { "kind": "drain", "max_wait_ms": 120000 }
    }"#;

    /// A certification pass must not author durable state.
    ///
    /// The composition-pin exemption lets a candidate launch boot without
    /// pinning its own restricted composition. That exemption would otherwise
    /// open a hole: a candidate that ADOPTED would persist a declaration under a
    /// composition no pin covers - trading a loud refusal for silent unpinned
    /// durable state, which is worse. The downstream app's own deploy doctrine says the
    /// same thing: candidate mode exists to gate effects.
    ///
    /// Asserted on the refusal's CONTENT, not merely that an error occurred: the
    /// message has to tell an operator which declaration caused it and what to do
    /// instead, or it is a dead end wearing a verdict's clothes.
    #[test]
    fn adoption_from_a_non_authoritative_launch_is_refused() {
        let refused =
            super::refuse_adoption_from_non_authoritative_launch(serde_json::json!("probe"));
        assert_eq!(refused.id, serde_json::json!("probe"));
        assert!(
            refused.result.is_none(),
            "a refusal must not also carry a result"
        );
        let error = refused
            .error
            .expect("a non-authoritative launch must be refused, not served");
        assert_eq!(error.code, -32602);
        assert!(
            error.message.contains("non-authoritative"),
            "the refusal must name the declaration that caused it: {}",
            error.message
        );
        assert!(
            error.message.contains("rehearsal store")
                || error.message.contains("authoritative (promoted) launch"),
            "the refusal must say what to do instead, or it is a dead end: {}",
            error.message
        );
    }

    #[test]
    fn the_adopt_payload_downstream_actually_sends_parses_and_converts() {
        let params: meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationParams =
            serde_json::from_str(DOWNSTREAM_ADOPT_PARAMS)
                .expect("the downstream app's production adopt payload must deserialize");
        assert_eq!(params.declaration_scope, "example-tool-policy");
        assert_eq!(params.declaration_revision, 1);

        // The whole payload converts in one step, which is why adoption inherits
        // meerkat's validation exactly rather than being reassembled here. If this
        // conversion ever tightens upstream, this fails on the downstream app's real payload
        // instead of on their 17th member.
        let request: meerkat_mob::AdoptMemberIdentityDeclaration = params
            .try_into()
            .expect("the production payload must convert to the domain request");
        let _ = request;
    }

    /// `wiring_custody` is absent from the downstream app's payload and must therefore
    /// default rather than being required: the field is `skip_serializing_if`
    /// external-managed on the wire, so a caller that omits it is saying
    /// "external managed", not "malformed".
    #[test]
    fn an_omitted_wiring_custody_defaults_instead_of_failing() {
        assert!(
            !DOWNSTREAM_ADOPT_PARAMS.contains("wiring_custody"),
            "fixture must keep exercising the omitted case"
        );
        let params: meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationParams =
            serde_json::from_str(DOWNSTREAM_ADOPT_PARAMS).expect("payload parses without it");
        assert!(params.wiring_custody.is_external_managed());
    }

    #[test]
    fn the_apply_payload_downstream_actually_sends_parses() {
        let params: meerkat_contracts::wire::MobApplyMemberToolDeclarationParams =
            serde_json::from_str(DOWNSTREAM_APPLY_PARAMS)
                .expect("the downstream app's production apply payload must deserialize");
        assert_eq!(params.mob_id, "example");
        assert_eq!(params.agent_identity, "identity:member-2");
        assert_eq!(params.expected_intent_revision, 3);

        // And it must survive the conversions the handler performs, so a payload
        // that parses but cannot become a domain request fails here rather than at
        // the downstream app's 17th member.
        let declaration: meerkat_mob::MemberToolDeclaration = params
            .declaration
            .try_into()
            .expect("the carried declaration must convert to the domain type");
        let _ = declaration;
        meerkat_mob::MemberToolMutationId::new(params.request_id)
            .expect("the downstream app's idempotency request_id scheme must be accepted");
    }

    /// `deny_unknown_fields` is what makes a typo fail loudly instead of arming
    /// nothing, which is exactly how the #337 carrier gap stayed invisible.
    #[test]
    fn an_unknown_field_is_refused_rather_than_ignored() {
        let with_typo = DOWNSTREAM_APPLY_PARAMS.replace(
            "\"expected_intent_revision\"",
            "\"expected_intent_revison\"",
        );
        let parsed: Result<meerkat_contracts::wire::MobApplyMemberToolDeclarationParams, _> =
            serde_json::from_str(&with_typo);
        assert!(
            parsed.is_err(),
            "a misspelled field must be refused, not silently defaulted"
        );
    }

    /// The category overrides are sent in full explicit form on purpose:
    /// inherit, disable and set are different facts. A wire that accepted an
    /// elided form would let a caller believe it had said something it had not.
    #[test]
    fn every_category_override_survives_the_round_trip() {
        let params: meerkat_contracts::wire::MobApplyMemberToolDeclarationParams =
            serde_json::from_str(DOWNSTREAM_APPLY_PARAMS).expect("payload parses");
        let reserialized = serde_json::to_value(&params).expect("wire params must reserialize");
        let overrides = reserialized
            .get("declaration")
            .and_then(|declaration| declaration.get("category_overrides"))
            .and_then(serde_json::Value::as_object)
            .expect("category_overrides must survive as an object");
        for category in [
            "builtins",
            "shell",
            "comms",
            "mob",
            "memory",
            "schedule",
            "workgraph",
            "image_generation",
            "web_search",
        ] {
            assert!(
                overrides.contains_key(category),
                "category override {category} was dropped on the way through the wire type"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// A roster row routes `meta/send_message` through the identity runtime
    /// while it is the identity's live member: Active, or Broken while Core
    /// holds that live member Broken. A broken identity with no live member
    /// and a dormant one do not.
    #[test]
    fn a_live_member_core_holds_broken_still_routes_through_the_identity_runtime() {
        use crate::identity_first::IdentityLifecycleState::{Active, Broken, Dormant};
        let status = |state, runtime_id: Option<&str>| crate::identity_first::IdentityStatus {
            identity: crate::identity_first::AgentIdentity::parse("personal:alice")
                .expect("an identity"),
            state,
            agent_runtime_id: runtime_id
                .map(|id| crate::identity_first::AgentRuntimeId::parse(id).expect("a runtime id")),
            session_id: None,
            member_id: None,
            profile: None,
            runtime_mode: None,
            addressability: crate::identity_first::AgentAddressability::Addressable,
            display_name: None,
            labels: std::collections::BTreeMap::new(),
            generation: None,
            intent_managed: false,
            convergence: None,
            wake_refusal: None,
            replaced_session: None,
            session_repair_required: None,
            customizer_tools_pending: None,
        };
        let live = Some("rt:personal:alice:1");
        assert!(status_owns_live_member(&status(Active, live)));
        assert!(status_owns_live_member(&status(Broken, live)));
        assert!(!status_owns_live_member(&status(Broken, None)));
        assert!(!status_owns_live_member(&status(Dormant, None)));
    }

    /// Pin for the send_message reservation contract: a generated
    /// `rt:{identity}:{generation}` alias must NEVER take the MobMember arm -
    /// even when a same-named member is physically present in the roster -
    /// because that arm reserves console interactions under the wire member
    /// id, which for a fenced alias would self-map the incarnation in
    /// `runtime_to_identity` (the console dispatch-mirroring defect). Both
    /// branches are pinned on the SAME rostered member: without identity
    /// authority the reserved-alias refusal wins, with it the Identity arm
    /// wins, and the roster probe is reached in neither case.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn generated_alias_never_resolves_to_the_mob_member_arm() -> Result<(), String> {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "send-message-generated-alias-pin-test"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        )
        .map_err(|err| format!("definition parses: {err:?}"))?;
        let runtime = crate::UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(std::sync::Arc::new(
                meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            ))
            .build()
            .await
            .map_err(|err| format!("runtime builds: {err:?}"))?;
        // A physically rostered member under the generated alias (the
        // identity runtime's own spawn shape). Roster presence must not
        // degrade the alias into the raw member plane.
        let mut spec = meerkat_mob::SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "rt:builder:0".to_string(),
            Some("You are Builder.".into()),
            None,
            None,
        );
        spec.identity = crate::member_comms_id::mob_member_id("rt:builder:0");
        runtime
            .mob_handle()
            .spawn_spec(spec)
            .await
            .map_err(|err| format!("generated-alias member spawns: {err:?}"))?;

        // No identity authority: the reserved-alias refusal, not the roster.
        if !matches!(
            resolve_send_message_target(&runtime, None, "rt:builder:0").await,
            SendMessageTarget::AuthorityUnavailable { .. }
        ) {
            return Err("a generated alias without identity authority must resolve \
                        AuthorityUnavailable, never MobMember"
                .to_string());
        }

        // Identity authority present: the identity plane is consulted BEFORE
        // the roster probe, so the same rostered alias resolves to its
        // durable identity.
        let identity_rt = std::sync::Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: std::sync::Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()
                        .map_err(|err| format!("in-memory continuity store: {err:?}"))?,
                ),
                lease_provider: std::sync::Arc::new(
                    crate::identity_first::LocalLeaseProvider::new(),
                ),
                runtime_instance_id: "send-message-generated-alias-pin-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            },
        ));
        let resolved =
            match resolve_send_message_target(&runtime, Some(&identity_rt), "rt:builder:0").await {
                SendMessageTarget::Identity { identity, .. } => identity,
                SendMessageTarget::MobMember => {
                    return Err(
                        "a generated alias under identity authority must never reach the \
                            roster plane"
                            .to_string(),
                    );
                }
                SendMessageTarget::AuthorityUnavailable { alias } => {
                    return Err(format!(
                        "a generated alias under identity authority must resolve Identity, got \
                     AuthorityUnavailable for {alias}"
                    ));
                }
            };
        if resolved.as_str() != "builder" {
            return Err(format!(
                "the alias must resolve to its DURABLE identity, got {}",
                resolved.as_str()
            ));
        }

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// `meta/run_flow` resolves every `role_targets` member id by the
    /// member-id alias rule before anything runs. Under identity authority a
    /// generated alias that is not the current incarnation is refused with
    /// the typed stale-binding refusal; without it the alias keeps its exact
    /// spelling, which names no member, so the mob refuses it as
    /// `not_a_member`. Neither collapses it onto its identity, and neither
    /// records a run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_flow_role_targets_resolve_generated_aliases_by_the_alias_rule()
    -> Result<(), String> {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "run-flow-role-targets-alias-rule-test"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true

[flows.review]
description = "review one change"

[flows.review.steps.review]
role = "worker"
message = "review"
"#,
        )
        .map_err(|err| format!("definition parses: {err:?}"))?;
        let runtime = crate::UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(std::sync::Arc::new(
                meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            ))
            .build()
            .await
            .map_err(|err| format!("runtime builds: {err:?}"))?;
        let identity_rt = std::sync::Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: std::sync::Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()
                        .map_err(|err| format!("in-memory continuity store: {err:?}"))?,
                ),
                lease_provider: std::sync::Arc::new(
                    crate::identity_first::LocalLeaseProvider::new(),
                ),
                runtime_instance_id: "run-flow-role-targets-alias-rule-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            },
        ));
        let params = serde_json::json!({
            "flow_id": "review",
            "params": {},
            "role_targets": { "worker": ["rt:rev-a1:2"] },
        });

        let stale = handle_run_flow(
            &runtime,
            Some(&identity_rt),
            serde_json::json!("stale"),
            &params,
        )
        .await;
        let stale_error = stale
            .error
            .ok_or("a stale generated alias must be refused")?;
        assert_eq!(stale_error.code, -32000, "{}", stale_error.message);
        assert_eq!(
            stale_error.data,
            Some(serde_json::json!({
                "kind": "stale_identity_runtime_binding",
                "identity": "rev-a1",
                "registered_runtime_member_id": null,
                "live_runtime_member_id": "rt:rev-a1:2",
                "registered_session_id": null,
                "live_session_id": null,
            }))
        );

        let exact = handle_run_flow(&runtime, None, serde_json::json!("exact"), &params).await;
        let exact_error = exact
            .error
            .ok_or("a generated alias without identity authority names no member")?;
        assert_eq!(exact_error.code, -32602, "{}", exact_error.message);
        assert_eq!(
            exact_error.data,
            Some(serde_json::json!({
                "kind": "flow_role_targets_refused",
                "flow_id": "review",
                "refusal": { "kind": "not_a_member", "role": "worker", "member": "rt:rev-a1:2" },
            }))
        );

        let runs = runtime
            .mob_handle()
            .list_runs(None)
            .await
            .map_err(|err| format!("list runs: {err:?}"))?;
        assert!(runs.is_empty(), "a refused binding records no run");

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// The AuthorityUnavailable reservation keys console events under the
    /// DURABLE identity derived from the alias, not the alias itself.
    #[test]
    fn authority_unavailable_reservation_identity_is_the_durable_identity() {
        assert_eq!(
            crate::member_comms_id::durable_identity_from_runtime_alias("rt:builder:0").as_deref(),
            Some("builder")
        );
        assert_eq!(
            crate::member_comms_id::durable_identity_from_runtime_alias("rt:review:singleton:3")
                .as_deref(),
            Some("review:singleton")
        );
    }

    /// Regression (the console dispatch-mirroring shape, this time on the
    /// `send_message` arm): a send addressed to a fenced alias whose identity
    /// authority is absent must key its console conversation on the DURABLE
    /// identity. Pre-fix the arm reserved with `(alias, alias)`, which
    /// self-mapped the incarnation in `runtime_to_identity` and put every
    /// frame on the `rt:builder:0` conversation instead of the `builder` one
    /// the console UI renders. The delivery itself still refuses - authority
    /// is genuinely unavailable - but the refusal frame must land on the
    /// durable conversation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn authority_unavailable_send_keys_console_events_on_the_durable_identity()
    -> Result<(), String> {
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "send-message-authority-unavailable-keying-test"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        )
        .map_err(|err| format!("definition parses: {err:?}"))?;
        let runtime = crate::UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(std::sync::Arc::new(
                meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            ))
            .build()
            .await
            .map_err(|err| format!("runtime builds: {err:?}"))?;
        // Roster the alias physically. This is what makes the test able to
        // SEE a resolution-order regression at the entry point: if the fenced
        // alias ever reached the MobMember arm, the send would be accepted by
        // the mob plane instead of refused, and its frames would key on the
        // incarnation.
        let mut spec = meerkat_mob::SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "rt:builder:0".to_string(),
            Some("You are Builder.".into()),
            None,
            None,
        );
        spec.identity = crate::member_comms_id::mob_member_id("rt:builder:0");
        runtime
            .mob_handle()
            .spawn_spec(spec)
            .await
            .map_err(|err| format!("generated-alias member spawns: {err:?}"))?;

        let response = handle_send_message(
            &runtime,
            None,
            serde_json::json!(1),
            &serde_json::json!({ "member_id": "rt:builder:0", "message": "ping" }),
        )
        .await;
        let error = response
            .error
            .ok_or_else(|| {
                "a fenced alias without identity authority must refuse delivery; acceptance means \
                 the send reached the raw mob plane"
                    .to_string()
            })?
            .message;
        if !error.contains("identity authority") {
            return Err(format!(
                "the refusal must be the identity-authority one, not a mob-plane error: {error}"
            ));
        }

        let events = runtime
            .console_events()
            .replay_all(None)
            .await
            .map_err(|err| format!("console replay: {err:?}"))?;
        let keyed: Vec<&str> = events
            .iter()
            .filter(|event| event.event_type == "interaction_failed")
            .map(|event| event.identity.as_str())
            .collect();
        if keyed != vec!["builder"] {
            return Err(format!(
                "the refusal frame must land on the durable `builder` conversation, not the \
                 incarnation: {keyed:?}"
            ));
        }
        if runtime
            .console_events()
            .response_phase_for_identity("builder")
            .await
            .is_none()
        {
            return Err(
                "the pending interaction must be reserved under the durable identity".to_string(),
            );
        }

        // Same entry point, authority present: the identity plane must take
        // the send. The identity is unregistered here, so the refusal comes
        // from the identity runtime - which is itself the evidence that the
        // roster probe was never reached, because the roster DOES hold this
        // alias and would have accepted it.
        let identity_rt = std::sync::Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: std::sync::Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()
                        .map_err(|err| format!("in-memory continuity store: {err:?}"))?,
                ),
                lease_provider: std::sync::Arc::new(
                    crate::identity_first::LocalLeaseProvider::new(),
                ),
                runtime_instance_id: "send-message-authority-unavailable-keying-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            },
        ));
        let response = handle_send_message(
            &runtime,
            Some(&identity_rt),
            serde_json::json!(2),
            &serde_json::json!({ "member_id": "rt:builder:0", "message": "ping again" }),
        )
        .await;
        if response.result.is_some() {
            return Err(
                "an unregistered identity must not be accepted through the roster plane"
                    .to_string(),
            );
        }

        // Whichever way the send resolved, no console frame may ever be keyed
        // on the incarnation: that is the self-map this arm exists to prevent.
        let events = runtime
            .console_events()
            .replay_all(None)
            .await
            .map_err(|err| format!("console replay: {err:?}"))?;
        if let Some(stray) = events.iter().find(|event| event.identity == "rt:builder:0") {
            return Err(format!(
                "no console frame may key on the incarnation, found {}: {}",
                stray.event_type, stray.identity
            ));
        }

        let _ = runtime.mob_handle().stop().await;
        Ok(())
    }

    /// Downstream app DX (2026-07-09): `cross_mob/peer_info` emits
    /// `transport_public_key` with the `ed25519:` scheme prefix; callers
    /// round-tripping it into `wire_local` had to strip it by hand.
    #[test]
    fn optional_pubkey_accepts_the_ed25519_prefixed_spelling() -> Result<(), String> {
        use base64::Engine as _;
        let key = [7u8; 32];
        let b64 = base64::engine::general_purpose::STANDARD.encode(key);
        let plain = serde_json::json!({ "remote_pubkey_b64": b64 });
        let prefixed = serde_json::json!({ "remote_pubkey_b64": format!("ed25519:{b64}") });
        assert_eq!(
            parse_optional_pubkey(&plain, "remote_pubkey_b64")?,
            Some(key)
        );
        assert_eq!(
            parse_optional_pubkey(&prefixed, "remote_pubkey_b64")?,
            Some(key)
        );
        Ok(())
    }

    #[test]
    fn extract_content_rejects_malformed_content_even_with_message_fallback()
    -> Result<(), Box<dyn std::error::Error>> {
        let params = serde_json::json!({
            "message": "fallback",
            "content": { "not": "a content input" },
        });

        let err = match extract_content(&params) {
            Ok(_) => {
                return Err(std::io::Error::other("content key must be authoritative").into());
            }
            Err(err) => err,
        };

        assert!(err.contains("invalid content"), "unexpected error: {err}");
        Ok(())
    }

    #[test]
    fn lifecycle_archive_cleanup_completed_accepts_post_disposal_cancel_race() {
        let error = "internal error: disposal completed but ArchiveSession failed: \
            session error: agent error: Internal error: runtime cancel-before-retire failed \
            for 019e3c52-0f1b-73d3-a5c7-4b21c2bbf131: Runtime not ready: running";

        assert!(lifecycle_archive_cleanup_completed(error));
    }

    #[test]
    fn lifecycle_archive_cleanup_completed_accepts_ambiguous_cleanup() {
        let error = "previous member cleanup ambiguous for member rt:review:singleton:0";

        assert!(lifecycle_archive_cleanup_completed(error));
    }

    #[test]
    fn lifecycle_archive_cleanup_completed_rejects_archive_not_found() {
        let error = "internal error: disposal completed but ArchiveSession failed: \
            session error: NotFound for registered runtime session";

        assert!(!lifecycle_archive_cleanup_completed(error));
    }
}

// ---------------------------------------------------------------------------
// Member declaration control plane (mob/*)
//
// Meerkat owns `adopt_member_identity_declaration` and
// `apply_member_tool_declaration` on the Mob handle and exposes them over
// rkat-rpc and rkat-rest. Meta linked the TYPES but reached neither, so a host
// composing through Meta could supply a compiled policy at init and then never
// bind a member to it: every member stayed Unmanaged and the installed provider
// governed nothing.
//
// These delegate straight to the canonical handle. No semantic validation is
// reimplemented here: adoption converts through meerkat-mob's own
// `TryFrom<MobAdoptMemberIdentityDeclarationParams>`, and the tool path reuses
// `MemberToolDeclaration`'s conversion and `MemberToolMutationId`'s constructor.
// Adoption stays a one-shot CAS against live intent state rather than becoming an
// init-time declaration list, because the precondition is the whole point.
// ---------------------------------------------------------------------------

/// Every method in the member-declaration control plane.
///
/// #343 registered these in two places and 0.8.21 shipped them DEAD on a third:
/// `http_console.rs` owns its own dispatch and its own `-32601`, so the methods
/// were reachable over stdin-RPC and unreachable over the console listener.
/// A downstream app hit that within an hour of release.
///
/// The lesson was not "there were three tables" but that a checklist assembled
/// from the sites one happens to find reads as exhaustive and is not. So the
/// family is a single registry every surface DERIVES from rather than a literal
/// pasted per surface. A fourth doorway can still forget to consult it, but it
/// can no longer disagree about the membership once it does.
pub(crate) const ADOPT_MEMBER_IDENTITY_DECLARATION: &str = "mob/adopt_member_identity_declaration";
pub(crate) const APPLY_MEMBER_TOOL_DECLARATION: &str = "mob/apply_member_tool_declaration";
pub(crate) const MEMBER_TOOL_DECLARATION: &str = "mob/member_tool_declaration";

pub(crate) const MEMBER_DECLARATION_METHODS: &[&str] = &[
    ADOPT_MEMBER_IDENTITY_DECLARATION,
    APPLY_MEMBER_TOOL_DECLARATION,
    MEMBER_TOOL_DECLARATION,
];

/// Whether a method belongs to this family.
#[must_use]
pub(crate) fn is_member_declaration_method(method: &str) -> bool {
    MEMBER_DECLARATION_METHODS.contains(&method)
}

/// Whether the method MUTATES durable member intent.
///
/// The read is not a mutation; adoption and apply both are. The console plane's
/// read_only refusal and its ABAC action mapping both hang off this, so getting it
/// wrong would either block a read or admit a write.
#[must_use]
pub(crate) fn is_member_declaration_mutating_method(method: &str) -> bool {
    matches!(
        method,
        ADOPT_MEMBER_IDENTITY_DECLARATION | APPLY_MEMBER_TOOL_DECLARATION
    )
}

/// One dispatcher for the whole family, so each surface has ONE arm instead of
/// three that can drift apart.
///
/// Takes `&MobHandle` because that is all these handlers ever used and it is the
/// one thing both planes can supply: `rpc.rs` passes `runtime.mob_handle()`, the
/// full-runtime console passes `runtime.handle()`. One implementation, two
/// doorways. Returns `None` for a method outside the family so a caller can fall
/// through to its own dispatch.
/// The typed refusal for adoption from a launch that does not speak for the
/// durable composition.
///
/// Extracted so the refusal can be asserted without fabricating a MobHandle: the
/// guard fires BEFORE dispatch, so there is nothing to build a handle for, and
/// inventing a test-only constructor on an upstream type to satisfy a signature
/// would be testing the wrong thing.
fn refuse_adoption_from_non_authoritative_launch(response_id: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message: "identity adoption refused: this launch declared itself non-authoritative \
                      for the mob composition (runtime_options.mob_composition.authority = \
                      candidate), and a candidate must not author durable state - the \
                      declaration would persist under a composition no pin covers. Adopt from \
                      the authoritative (promoted) launch, or certify adoption against a \
                      rehearsal store."
                .to_string(),
            data: None,
        }),
    }
}

pub(crate) async fn handle_member_declaration_rpc(
    handle: &meerkat_mob::MobHandle,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    speaks_for_composition: bool,
    method: &str,
    response_id: Value,
    params: &Value,
) -> Option<JsonRpcResponse> {
    match method {
        ADOPT_MEMBER_IDENTITY_DECLARATION => {
            // A launch that does not speak for the durable composition must not
            // AUTHOR durable state. Adoption writes a declaration that outlives
            // the process, and the pin exemption means no pin covers the
            // composition it was written under - so allowing it would trade the
            // 929-respawn refusal for silent unpinned durable state, which is
            // worse. A certification pass certifies; it does not author.
            //
            // If adoption behaviour itself needs certifying, the honest shape is
            // a rehearsal store, never the production store through an exemption.
            if !speaks_for_composition {
                return Some(refuse_adoption_from_non_authoritative_launch(response_id));
            }
            Some(handle_adopt_member_identity_declaration(handle, response_id, params).await)
        }
        APPLY_MEMBER_TOOL_DECLARATION => {
            Some(handle_apply_member_tool_declaration(handle, response_id, params).await)
        }
        MEMBER_TOOL_DECLARATION => Some(
            handle_member_tool_declaration(handle, identity_runtime, response_id, params).await,
        ),
        _ => None,
    }
}

/// Mirror a domain identity back to the PUBLIC alias on the way out.
///
/// Input translation alone was half a fix. The roster is keyed by the comms-safe
/// encoded id, so every outbound identity - the read's own `agent_identity` AND
/// the `agent_identity` inside every `WireIdentityConvergenceStatus` - would
/// otherwise carry `mk--rt_c...`. That is the RESERVED namespace which
/// `validate_public_rpc_member_aliases` refuses as INPUT, so a caller could not
/// even feed our own output back to us, and it exposes an internal spelling that
/// exists to keep ABAC unbypassable.
fn public_member_alias(identity: &str) -> String {
    crate::member_comms_id::runtime_alias_str(identity).into_owned()
}

/// Decode the identity inside an outbound convergence status.
///
/// Applied on ALL THREE methods, because all three return one.
fn mirror_convergence(
    mut wire: meerkat_contracts::wire::WireIdentityConvergenceStatus,
) -> meerkat_contracts::wire::WireIdentityConvergenceStatus {
    wire.agent_identity = public_member_alias(&wire.agent_identity);
    wire
}

/// Render a `MobError` with the same typed code and structured data that
/// meerkat's own RPC surface produces.
///
/// meerkat-rpc reaches this through a `MobWireErrorSource` trait that is private
/// to its handler module, so the mapping is reproduced from `MobError`'s public
/// accessors, once, in `mob_error_wire`, which a keyed send's refusal renders
/// through too. Kept deliberately identical: a host that moves between rkat-rpc
/// and the Meta gateway must classify the same failure the same way, and these
/// are the first typed-error responses in this module - the surrounding
/// handlers hand-shape `-32000` with an ad-hoc `data.kind`.
pub(crate) fn mob_declaration_error(
    response_id: Value,
    error: &meerkat_mob::MobError,
) -> JsonRpcResponse {
    let (code, data) = match crate::identity_first::runtime::mob_error_wire(error) {
        Ok(wire) => wire,
        // Fail closed rather than downgrade to an untyped error: a caller
        // that cannot see WHICH mob error this was is better served by an
        // explicit internal error than by a plausible-looking -32602.
        Err(serialize_error) => {
            return JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32603,
                    message: format!("failed to serialize mob error detail: {serialize_error}"),
                    data: None,
                }),
            };
        }
    };
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: error.to_string(),
            data,
        }),
    }
}

/// Declaration writes have no identity authority to pin a generated runtime
/// alias to its current incarnation. They take the durable identity and refuse
/// a generated alias instead of collapsing its generation onto whichever
/// incarnation holds the durable row now (#366).
fn refuse_generated_member_alias(
    response_id: &Value,
    agent_identity: &str,
) -> Option<JsonRpcResponse> {
    let decoded = crate::member_comms_id::runtime_alias_str(agent_identity);
    crate::member_comms_id::is_reserved_generated_alias(&decoded).then(|| {
        declaration_invalid_params(
            response_id.clone(),
            format!(
                "generated member alias requires identity authority; address the member by its durable identity: {decoded}"
            ),
        )
    })
}

fn declaration_invalid_params(response_id: Value, message: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message,
            data: None,
        }),
    }
}

fn declaration_result_response(response_id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result: Some(result),
        error: None,
    }
}

/// Refuse a payload aimed at a different mob.
///
/// The handle overwrites `request.mob_id` with its own, so a mismatched value
/// would otherwise be silently retargeted at THIS mob and reported as success.
/// meerkat's RPC surface selects state by the supplied mob_id, so a mismatch
/// there is a lookup; a gateway serves exactly one mob, so here it is a caller
/// error and saying so is the only way the caller can find out.
fn reject_foreign_mob(
    handle: &meerkat_mob::MobHandle,
    response_id: &Value,
    supplied: &str,
) -> Option<JsonRpcResponse> {
    let own = handle.mob_id().to_string();
    if supplied == own {
        return None;
    }
    Some(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id.clone(),
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message: format!("request targets mob '{supplied}' but this gateway serves '{own}'"),
            data: Some(serde_json::json!({
                "kind": "foreign_mob_target",
                "requested_mob_id": supplied,
                "gateway_mob_id": own,
            })),
        }),
    })
}

/// `mob/adopt_member_identity_declaration`
///
/// Establish revision 1 for an existing member under an explicit
/// expected-absent precondition. The whole wire payload converts through
/// meerkat-mob, so the precondition and every field constraint are enforced by
/// the same code rkat-rpc uses.
pub(super) async fn handle_adopt_member_identity_declaration(
    handle: &meerkat_mob::MobHandle,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let params: meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationParams =
        match serde_json::from_value(params.clone()) {
            Ok(params) => params,
            Err(error) => return declaration_invalid_params(response_id, error.to_string()),
        };
    if let Some(rejection) = reject_foreign_mob(handle, &response_id, &params.mob_id) {
        return rejection;
    }
    // Same translation for adoption. Done on the wire struct BEFORE meerkat's
    // whole-params conversion, because that conversion takes the field as given -
    // pre-translating keeps one boundary rule for all three methods instead of
    // needing an upstream change.
    if let Some(rejection) = refuse_generated_member_alias(&response_id, &params.agent_identity) {
        return rejection;
    }
    let mut params = params;
    params.agent_identity =
        crate::member_comms_id::mob_member_id_str(&params.agent_identity).into_owned();
    let request: meerkat_mob::AdoptMemberIdentityDeclaration = match params.try_into() {
        Ok(request) => request,
        Err(error) => return declaration_invalid_params(response_id, error.to_string()),
    };
    match handle.adopt_member_identity_declaration(request).await {
        Ok(result) => {
            let wire = meerkat_contracts::wire::MobAdoptMemberIdentityDeclarationResult {
                adoption: result.adoption.to_wire(),
                convergence: mirror_convergence(result.convergence.to_wire()),
            };
            match serde_json::to_value(wire) {
                Ok(value) => declaration_result_response(response_id, value),
                Err(error) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32603,
                        message: format!("failed to serialize adoption result: {error}"),
                        data: None,
                    }),
                },
            }
        }
        Err(error) => mob_declaration_error(response_id, &error),
    }
}

/// `mob/member_tool_declaration`
///
/// Read the live member-tool declaration and its desired intent revision.
///
/// Included at the downstream app's request and on the lead's instruction: without it the
/// apply CAS is guess-and-retry, because `expected_intent_revision` can only be
/// learned by reading. Delegates to `identity_intent` and
/// `identity_convergence_status` on the handle; the Missing-convergence fallback
/// mirrors meerkat's own handler so a member with intent but no observed
/// convergence yet reports the desired revision rather than failing.
pub(super) async fn handle_member_tool_declaration(
    handle: &meerkat_mob::MobHandle,
    identity_runtime: Option<&std::sync::Arc<crate::identity_first::IdentityRuntime>>,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let params: meerkat_contracts::wire::MobMemberToolDeclarationParams =
        match serde_json::from_value(params.clone()) {
            Ok(params) => params,
            Err(error) => return declaration_invalid_params(response_id, error.to_string()),
        };
    if let Some(rejection) = reject_foreign_mob(handle, &response_id, &params.mob_id) {
        return rejection;
    }
    // A read under the member-id alias rule: where an identity runtime is
    // present, a stale runtime alias is refused first, as `meta/get_member`
    // refuses it, before the supplied id maps to the durable row below.
    if let Some(response) = stale_runtime_alias_error_response(
        identity_runtime,
        &params.agent_identity,
        response_id.clone(),
    )
    .await
    {
        return response;
    }
    // The wire carries a public member ALIAS; the roster is keyed by the comms-safe
    // ENCODED id. `member_comms_id::mob_member_id` is the public translation and
    // exists precisely for this boundary. #343 skipped it and used
    // `AgentIdentity::from(alias)` directly, so on a live identity-first mob every
    // call answered "mob member not found": the alias missed, the runtime alias
    // missed, and the encoded spelling is refused outright by
    // `validate_public_rpc_member_aliases` so ABAC cannot be bypassed by it. The
    // methods were reachable and unable to name anything. A read also takes the
    // current runtime alias `status_identity` returns: it names the durable row
    // (#366).
    let identity = crate::member_comms_id::roster_member_id_for_supplied_id(&params.agent_identity);
    let record = match handle.identity_intent(&identity).await {
        Ok(meerkat_mob::identity::IdentityStoredObservation::Valid(record)) => record,
        Ok(meerkat_mob::identity::IdentityStoredObservation::Missing) => {
            return declaration_invalid_params(
                response_id,
                "member has no durable identity intent".to_string(),
            );
        }
        Ok(
            meerkat_mob::identity::IdentityStoredObservation::Unsupported { detail, .. }
            | meerkat_mob::identity::IdentityStoredObservation::Malformed { detail, .. },
        ) => {
            return JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32603,
                    message: detail,
                    data: None,
                }),
            };
        }
        Err(error) => return mob_declaration_error(response_id, &error),
    };
    let declaration = match &record.intent {
        meerkat_mob::identity::IdentityIntent::Present { member, .. } => {
            member.material.member_tool_declaration()
        }
        meerkat_mob::identity::IdentityIntent::Absent { .. } => {
            return declaration_invalid_params(
                response_id,
                "member desired presence is absent".to_string(),
            );
        }
    };
    let convergence = match handle.identity_convergence_status(&identity).await {
        Ok(meerkat_mob::identity::IdentityStoredObservation::Valid(status)) => status.to_wire(),
        // A member can hold intent before any convergence has been observed.
        // Reporting the desired revision is what meerkat does, and refusing here
        // would make the read unusable in exactly the window that follows an
        // adoption - which is when Phase B needs it.
        Ok(meerkat_mob::identity::IdentityStoredObservation::Missing) => {
            meerkat_mob::identity::IdentityConvergenceStatus {
                identity: identity.clone(),
                intent_revision: Some(record.intent_revision),
                active_intent_revision: None,
                lease_epoch: None,
                decision: None,
                observed_at_ms: 0,
                detail: None,
            }
            .to_wire()
        }
        Ok(
            meerkat_mob::identity::IdentityStoredObservation::Unsupported { detail, .. }
            | meerkat_mob::identity::IdentityStoredObservation::Malformed { detail, .. },
        ) => {
            return JsonRpcResponse {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: response_id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32603,
                    message: format!("identity convergence status is unavailable: {detail}"),
                    data: None,
                }),
            };
        }
        Err(error) => return mob_declaration_error(response_id, &error),
    };
    let wire = meerkat_contracts::wire::MobMemberToolDeclarationResult {
        mob_id: handle.mob_id().to_string(),
        agent_identity: public_member_alias(identity.as_ref()),
        desired_intent_revision: record.intent_revision,
        declaration: declaration.to_wire(),
        convergence: mirror_convergence(convergence),
    };
    match serde_json::to_value(wire) {
        Ok(value) => declaration_result_response(response_id, value),
        Err(error) => JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: response_id,
            result: None,
            error: Some(JsonRpcError {
                code: -32603,
                message: format!("failed to serialize member tool declaration: {error}"),
                data: None,
            }),
        },
    }
}

/// `mob/apply_member_tool_declaration`
///
/// Atomically update only the tool portion of an existing durable member intent.
///
/// Assembled field by field because meerkat-mob exposes no whole-params
/// conversion for this one, so this mirrors meerkat-rpc's handler exactly. The
/// semantic checks still live upstream (`MemberToolDeclaration`'s conversion and
/// `MemberToolMutationId::new`); only the assembly is duplicated, which is worth
/// replacing with a shared `TryFrom` in meerkat-mob.
pub(super) async fn handle_apply_member_tool_declaration(
    handle: &meerkat_mob::MobHandle,
    response_id: Value,
    params: &Value,
) -> JsonRpcResponse {
    let params: meerkat_contracts::wire::MobApplyMemberToolDeclarationParams =
        match serde_json::from_value(params.clone()) {
            Ok(params) => params,
            Err(error) => return declaration_invalid_params(response_id, error.to_string()),
        };
    if let Some(rejection) = reject_foreign_mob(handle, &response_id, &params.mob_id) {
        return rejection;
    }
    if let Some(rejection) = refuse_generated_member_alias(&response_id, &params.agent_identity) {
        return rejection;
    }
    let declaration: meerkat_mob::MemberToolDeclaration = match params.declaration.try_into() {
        Ok(declaration) => declaration,
        Err(error) => {
            return declaration_invalid_params(
                response_id,
                std::string::ToString::to_string(&error),
            );
        }
    };
    let request_id = match meerkat_mob::MemberToolMutationId::new(params.request_id) {
        Ok(request_id) => request_id,
        Err(error) => {
            return declaration_invalid_params(
                response_id,
                std::string::ToString::to_string(&error),
            );
        }
    };
    let request = meerkat_mob::ApplyMemberToolDeclaration {
        mob_id: handle.mob_id().clone(),
        agent_identity: crate::member_comms_id::mob_member_id(&params.agent_identity),
        request_id,
        expected_intent_revision: params.expected_intent_revision,
        declaration,
        convergence: params.convergence.into(),
    };
    match handle.apply_member_tool_declaration(request).await {
        Ok(result) => {
            let wire = meerkat_contracts::wire::MobApplyMemberToolDeclarationResult {
                commit: result.commit.to_wire(),
                convergence: mirror_convergence(result.convergence.to_wire()),
            };
            match serde_json::to_value(wire) {
                Ok(value) => declaration_result_response(response_id, value),
                Err(error) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32603,
                        message: format!("failed to serialize tool declaration result: {error}"),
                        data: None,
                    }),
                },
            }
        }
        Err(error) => mob_declaration_error(response_id, &error),
    }
}
