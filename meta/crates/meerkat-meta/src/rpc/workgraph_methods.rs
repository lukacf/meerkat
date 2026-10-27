//! `meta/workgraph/*` method dispatch, shared by the unified stdin RPC and
//! the console RPC surfaces (docs/design/workgraph-wire-contract.md).
//!
//! Params deserialize into meerkat's typed requests and results serialize the
//! typed results verbatim, with these meta-side rules:
//! - `realm_id` is never accepted over the wire — the service is scoped to
//!   this runtime's realm (the mob definition id) at construction.
//! - The authority witness (`authority_projection`) for `policy/escalate`
//!   and `attention/reassign` is fetched server-side from `binding_id`;
//!   wire-supplied witnesses are rejected (they are unforgeable by design).
//! - Attention targets accept an additional `{kind:"identity", identity}`
//!   form, lowered through `meerkat_mob::lower_agent_identity_attention_target`
//!   with this runtime's mob id, and `{kind:"lowered_owner", owner_key}` as
//!   an alias of `owner` — the spelling results serialize, so a result's
//!   `attention.target` round-trips verbatim into params. Session targets
//!   that resolve to a member (roster, or the shared session store's
//!   member-binding metadata) are ALSO lowered to the member's owner form
//!   before the write (`goal/create`, `attention/reassign`) — see the
//!   normalize-at-write section of [`crate::workgraph_admission`]'s module
//!   docs.
//! - `goal/create`, `attention/resume` and `attention/reassign` refuse to
//!   give a target a second Active-or-Paused binding. Upstream ask 25 refuses
//!   the Active-vs-Active, identical-target-key half of that in the store
//!   itself; meta still owns the paused, cross-spelling and
//!   session-identity-alias cases, where an admitted duplicate makes the turn
//!   overlay arbitrate newest-binding-wins and silently starve the losing
//!   goal. The check lives in [`crate::workgraph_admission::WorkGraphAdmission`] —
//!   shared with the agent tool plane's `workgraph_attention_reassign` —
//!   which resolves session↔identity target aliases through the mob roster
//!   and the shared session store's member-binding metadata, and serializes
//!   every check-then-act window on one runtime-wide gate (plus a
//!   cross-process sidecar lock for SQLite-backed stores).
//! - Goal/attention methods only accept the service's default namespace —
//!   upstream turn overlays resolve nowhere else.
//! - The `attention/list` `status` filter accepts the SDKs' bare-string
//!   spelling beside upstream's internally-tagged object form.
//! - `events` already carries meerkat's TRANSACTIONAL `WorkGraphFact` rows:
//!   `WorkGraphEvent::facts` serializes verbatim through this dispatch (it is
//!   only elided when empty). `facts` is its projected sibling - the same
//!   ledger page reduced to
//!   [`WorkGraphFactEnvelope`](crate::workgraph_events::WorkGraphFactEnvelope)
//!   rows, which is exactly the shape the fact stream publishes. It exists so
//!   a reconnecting stream client has a catch-up read in the stream's own
//!   shape instead of every SDK re-deriving the projection. Facts are LOSSY
//!   WAKE ACCELERANTS: they carry identifiers only, and a client reconciles
//!   authoritative state through `get`/`list`/`ready`/`snapshot`. Nothing on
//!   this surface may treat a fact as authority.

use meerkat::{
    AddEvidenceRequest, AttentionListRequest, AttentionPauseRequest, AttentionProjectionRequest,
    AttentionPruneRequest, AttentionReassignRequest, AttentionResumeRequest,
    BreakGlassAttentionReassignRequest, ClaimWorkItemRequest, CloseWorkItemRequest,
    CreateWorkItemRequest, GoalAttentionTarget, GoalConfirmRequest, GoalCreateRequest,
    GoalRequestCloseRequest, GoalStatusRequest, LinkWorkItemsRequest, PolicyEscalateRequest,
    ReadyWorkFilter, ReleaseWorkItemRequest, UpdateWorkItemRequest, WorkAttentionBindingId,
    WorkGraphError, WorkGraphEventFilter, WorkGraphIdParams, WorkGraphService,
    WorkGraphSnapshotFilter, WorkItemFilter, WorkItemId, WorkNamespace, WorkOwnerKey, WorkStatus,
};
use serde_json::Map;

use crate::workgraph_admission::{WorkGraphAdmission, WorkGraphAdmissionError};

use super::*;

/// Read methods (console ABAC action `workgraph.view`).
pub(crate) const WORKGRAPH_READ_METHODS: &[&str] = &[
    "meta/workgraph/snapshot",
    "meta/workgraph/list",
    "meta/workgraph/get",
    "meta/workgraph/ready",
    "meta/workgraph/events",
    "meta/workgraph/facts",
    "meta/workgraph/attention/list",
    "meta/workgraph/goal/status",
];

/// Mutating methods (console ABAC action `workgraph.manage`; additionally
/// gated by the console read-only switch).
pub(crate) const WORKGRAPH_MUTATE_METHODS: &[&str] = &[
    "meta/workgraph/create",
    "meta/workgraph/update",
    "meta/workgraph/claim",
    "meta/workgraph/release",
    "meta/workgraph/close",
    "meta/workgraph/block",
    "meta/workgraph/link",
    "meta/workgraph/evidence/add",
    "meta/workgraph/policy/escalate",
    "meta/workgraph/goal/create",
    "meta/workgraph/goal/confirm",
    "meta/workgraph/goal/request_close",
    "meta/workgraph/attention/pause",
    "meta/workgraph/attention/resume",
    "meta/workgraph/attention/reassign",
    "meta/workgraph/attention/prune",
];

/// Console-surface-only mutating methods (ABAC `workgraph.manage` + the
/// read-only switch, like [`WORKGRAPH_MUTATE_METHODS`]). Break-glass
/// reassignment is an operator recovery act (upstream ask 23): its principal
/// is the AUTHENTICATED console principal, so the method does not exist on
/// the host stdin surface, which carries no wire principal to attribute.
pub(crate) const WORKGRAPH_CONSOLE_MUTATE_METHODS: &[&str] =
    &["meta/workgraph/attention/break_glass_reassign"];

/// Whether `method` belongs to the workgraph RPC namespace (known or not).
pub(crate) fn is_workgraph_method(method: &str) -> bool {
    method.starts_with("meta/workgraph/")
}

pub(crate) fn is_workgraph_read_method(method: &str) -> bool {
    WORKGRAPH_READ_METHODS.contains(&method)
}

pub(crate) fn is_workgraph_mutating_method(method: &str) -> bool {
    WORKGRAPH_MUTATE_METHODS.contains(&method) || WORKGRAPH_CONSOLE_MUTATE_METHODS.contains(&method)
}

fn invalid_params(message: impl std::fmt::Display) -> JsonRpcError {
    JsonRpcError {
        code: -32602,
        message: format!("Invalid params: {message}"),
        data: None,
    }
}

fn method_not_found() -> JsonRpcError {
    JsonRpcError {
        code: -32601,
        message: "Method not found".to_string(),
        data: None,
    }
}

pub(crate) fn workgraph_unavailable_error() -> JsonRpcError {
    JsonRpcError {
        code: WORKGRAPH_UNAVAILABLE_CODE,
        message: "workgraph is not configured on this runtime".to_string(),
        data: Some(serde_json::json!({ "kind": "workgraph_unavailable" })),
    }
}

/// Upstream `validate_workgraph_attention_projection_current` spells a stale
/// authority witness as a generic `InvalidTransition` with this message
/// prefix (meerkat 0.8.22, meerkat-workgraph/src/tool_surface.rs). The
/// variant carries no structure to match on, so the prefix is pinned by
/// `stale_attention_witness_maps_to_conflict`.
const STALE_ATTENTION_WITNESS_PREFIX: &str = "stale WorkGraph attention projection";

fn workgraph_conflict(detail: String) -> JsonRpcError {
    JsonRpcError {
        code: WORKGRAPH_CONFLICT_CODE,
        message: format!("workgraph conflict: {detail}"),
        data: Some(serde_json::json!({
            "kind": "workgraph_conflict",
            "detail": detail,
        })),
    }
}

/// Map a WorkGraph domain error onto the wire taxonomy: CAS conflicts and
/// stale authority witnesses (a retryable race — the binding or item moved
/// between witness fetch and use) get the typed conflict code, domain-level
/// input rejections read as invalid params, everything else is a workgraph
/// error with full detail.
/// `data.kind` of the -32602 refusal for a goal, binding or reassignment
/// whose target names a member of another mob (its bindings live only in
/// that mob's realm; see `crate::workgraph_realm`).
pub(crate) const ATTENTION_TARGET_REALM_MISMATCH_KIND: &str = "attention_target_realm_mismatch";

fn workgraph_error_to_rpc(error: WorkGraphError) -> JsonRpcError {
    let detail = error.to_string();
    match error {
        WorkGraphError::StaleRevision { .. } | WorkGraphError::Conflict(_) => {
            workgraph_conflict(detail)
        }
        WorkGraphError::InvalidTransition(ref message)
            if message.starts_with(STALE_ATTENTION_WITNESS_PREFIX) =>
        {
            workgraph_conflict(detail)
        }
        WorkGraphError::InvalidInput(_) => invalid_params(detail),
        // A member-bound target outside its mob realm: refused at the call
        // (meerkat classifies it InvalidArguments) with the typed fields so
        // a client can name the realm the binding must be created in.
        WorkGraphError::AttentionTargetRealmMismatch {
            owner_key,
            mob_id,
            required_realm_id,
            realm_id,
        } => JsonRpcError {
            code: -32602,
            message: format!("Invalid params: {detail}"),
            data: Some(serde_json::json!({
                "kind": ATTENTION_TARGET_REALM_MISMATCH_KIND,
                "owner_key": owner_key,
                "mob_id": mob_id,
                "required_realm_id": required_realm_id,
                "realm_id": realm_id,
            })),
        },
        _ => JsonRpcError {
            code: WORKGRAPH_ERROR_CODE,
            message: detail.clone(),
            data: Some(serde_json::json!({
                "kind": "workgraph_error",
                "detail": detail,
            })),
        },
    }
}

fn to_result_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Clone the request params as an object map. `null`/absent params are an
/// empty object (every read filter is fully optional); anything else is a
/// params error.
fn params_object(params: &Value) -> Result<Map<String, Value>, JsonRpcError> {
    match params {
        Value::Null => Ok(Map::new()),
        Value::Object(map) => Ok(map.clone()),
        _ => Err(invalid_params("params must be a JSON object")),
    }
}

fn parse_request<T: serde::de::DeserializeOwned>(
    object: Map<String, Value>,
) -> Result<T, JsonRpcError> {
    serde_json::from_value(Value::Object(object)).map_err(invalid_params)
}

fn parse_binding_id(object: &Map<String, Value>) -> Result<WorkAttentionBindingId, JsonRpcError> {
    let raw = object
        .get("binding_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_params("binding_id must be a non-empty string"))?;
    WorkAttentionBindingId::new(raw).map_err(invalid_params)
}

fn parse_expected_revision(object: &Map<String, Value>) -> Result<u64, JsonRpcError> {
    object
        .get("expected_revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_params("expected_revision must be a non-negative integer"))
}

fn parse_namespace(object: &Map<String, Value>) -> Result<Option<WorkNamespace>, JsonRpcError> {
    match object.get("namespace") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| invalid_params(format!("namespace is invalid: {error}"))),
    }
}

/// Both SDKs write the attention-list `status` filter as a bare string
/// ("active"), but upstream `WorkAttentionStatus` is internally tagged
/// (`#[serde(tag = "state")]`) and rejects plain strings — the filter could
/// never match anything. Map the four bare spellings onto the tagged form
/// ("paused" carries no `until`); tagged objects pass through verbatim.
fn normalize_attention_status_param(object: &mut Map<String, Value>) -> Result<(), JsonRpcError> {
    let Some(value) = object.get("status") else {
        return Ok(());
    };
    let Some(text) = value.as_str() else {
        return Ok(());
    };
    match text {
        "active" | "paused" | "superseded" | "stopped" => {
            let tagged = serde_json::json!({ "state": text });
            object.insert("status".to_string(), tagged);
            Ok(())
        }
        other => Err(invalid_params(format!(
            "status '{other}' is unknown (allowed: active, paused, superseded, stopped)"
        ))),
    }
}

/// Goal/attention methods must stay in the service's default namespace:
/// upstream turn-overlay resolution lists attention with `namespace: None`
/// — the default only (meerkat 0.8.22, meerkat/src/surface.rs,
/// `resolve_workgraph_attention_projection_for_session`) — so a goal or
/// binding filed anywhere else is silently inert: it never reaches its
/// member. Reject rather than accept-and-strand.
///
/// ITEM-LEVEL PASSTHROUGH IS GONE AS OF 0.8.22 and this comment used to say
/// otherwise. One `WorkGraphService` owns ONE IMMUTABLE namespace grant -
/// upstream states it directly: "a namespace grant authorizes exactly one
/// immutable namespace" (meerkat-workgraph/src/service.rs). Meta installs a
/// single service scoped to the canonical mob realm + default namespace, so an
/// item filed into a sidecar namespace is refused by the grant, typed and
/// fail-closed, rather than passed through. That refusal is CORRECT: a sidecar
/// namespace needs its own service, grant and surface, not a wildcard on this
/// one. Never widen the grant to restore the old passthrough.
fn reject_non_default_namespace(
    service: &WorkGraphService,
    object: &Map<String, Value>,
) -> Result<(), JsonRpcError> {
    let Some(namespace) = parse_namespace(object)? else {
        return Ok(());
    };
    if namespace == *service.default_namespace() {
        return Ok(());
    }
    Err(invalid_params(format!(
        "namespace '{}' is not accepted on goal/attention methods: turn attention overlays \
         resolve only in the default namespace '{}', so a goal filed elsewhere would never \
         reach its member",
        namespace.as_str(),
        service.default_namespace().as_str(),
    )))
}

/// Resolve a goal/attention target, accepting the meta-only
/// `{kind:"identity", identity}` form beside upstream `session`/`owner`,
/// plus `lowered_owner` — the spelling every RESULT serializes (the stored
/// binding target is a `WorkAttentionTarget`, whose owner arm is
/// `LoweredOwner`) — so a result's `attention.target` round-trips verbatim
/// back into `goal/create`/`attention/reassign` params.
fn resolve_goal_target(
    mob_id: &meerkat_mob::MobId,
    value: &Value,
) -> Result<GoalAttentionTarget, JsonRpcError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_params("target must be a JSON object"))?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "identity" => {
            let identity = object
                .get("identity")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| invalid_params("target.identity must be a non-empty string"))?;
            meerkat_mob::lower_agent_identity_attention_target(
                mob_id,
                &meerkat_mob::AgentIdentity::from(identity),
            )
            .map_err(invalid_params)
        }
        "session" | "owner" => serde_json::from_value(value.clone())
            .map_err(|error| invalid_params(format!("target is invalid: {error}"))),
        // Alias of `owner` with the same `owner_key` payload shape.
        "lowered_owner" => {
            let mut object = object.clone();
            object.insert("kind".to_string(), Value::String("owner".to_string()));
            serde_json::from_value(Value::Object(object))
                .map_err(|error| invalid_params(format!("target is invalid: {error}")))
        }
        other => Err(invalid_params(format!(
            "target.kind '{other}' is unsupported (allowed: session, owner, lowered_owner, \
             identity)"
        ))),
    }
}

/// Synthesize the confirm evidence for an evidence-less wire call: the kind
/// literal the goal's completion policy admits (mirrors the machine's
/// `required_confirmation_evidence_kind` vocabulary), keyed on the binding.
/// The service still validates the confirming principal and stamps the
/// canonical confirmation classification.
fn default_confirm_evidence(
    policy: &meerkat::WorkCompletionPolicy,
    binding_id: &WorkAttentionBindingId,
) -> meerkat::WorkEvidenceRef {
    let kind = match policy {
        meerkat::WorkCompletionPolicy::SelfAttest => "self_attest",
        meerkat::WorkCompletionPolicy::HostConfirmed => "host_confirmation",
        meerkat::WorkCompletionPolicy::PrincipalConfirmed => "principal_confirmation",
        meerkat::WorkCompletionPolicy::Supervisor { .. } => "supervisor_confirmation",
        meerkat::WorkCompletionPolicy::ReviewerQuorum { .. } => "reviewer_confirmation",
    };
    meerkat::WorkEvidenceRef {
        kind: kind.to_string(),
        id: binding_id.as_str().to_string(),
        label: None,
        summary: None,
        confirmation_kind: None,
        confirming_owner_key: None,
        // Confirmation evidence is keyed on the ATTENTION binding; a work
        // EXECUTION binding (meerkat 0.8.16 flow-execution provenance) does
        // not exist on this path.
        execution_binding_id: None,
    }
}

/// Parse wire-supplied `goal/confirm` evidence. Only provenance fields are
/// accepted — the canonical confirmation classification
/// (`confirmation_kind`/`confirming_owner_key`) is stamped by the service
/// from the completion policy + trusted principal, so wire callers cannot
/// mint it directly.
fn parse_confirm_evidence(value: &Value) -> Result<meerkat::WorkEvidenceRef, JsonRpcError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_params("evidence must be a JSON object"))?;
    for key in object.keys() {
        if !matches!(key.as_str(), "kind" | "id" | "label" | "summary") {
            return Err(invalid_params(format!(
                "evidence.{key} is not accepted (allowed: kind, id, label, summary; \
                 confirmation classification is stamped server-side)"
            )));
        }
    }
    let field = |name: &str| -> Result<String, JsonRpcError> {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| invalid_params(format!("evidence.{name} must be a non-empty string")))
    };
    let optional = |name: &str| -> Result<Option<String>, JsonRpcError> {
        match object.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_str()
                .map(|value| Some(value.to_string()))
                .ok_or_else(|| invalid_params(format!("evidence.{name} must be a string"))),
        }
    };
    Ok(meerkat::WorkEvidenceRef {
        kind: field("kind")?,
        id: field("id")?,
        label: optional("label")?,
        summary: optional("summary")?,
        confirmation_kind: None,
        confirming_owner_key: None,
        // The wire surface does not carry an execution binding; typed
        // execution provenance stays server-minted (0.8.16 vocabulary).
        execution_binding_id: None,
    })
}

/// Fetch the live attention projection for `binding_id` — the server-side
/// authority witness `policy/escalate` and `attention/reassign` require.
async fn fetch_authority_projection(
    service: &WorkGraphService,
    binding_id: WorkAttentionBindingId,
    namespace: Option<WorkNamespace>,
) -> Result<meerkat::AttentionContextProjection, JsonRpcError> {
    let result = service
        .attention_projection(AttentionProjectionRequest {
            binding_id,
            realm_id: None,
            namespace,
        })
        .await
        .map_err(workgraph_error_to_rpc)?;
    Ok(result.projection)
}

/// `attention/reassign` demands `can_link_derived_from`, which the authority
/// machine derives only for coordinate-mode bindings — so for every other
/// mode the server-side witness ALWAYS fails the upstream authority check,
/// as a generic `InvalidInput`. Name the binding's mode and the restriction
/// so the caller learns why instead of retrying a request that can never
/// succeed. Everything else keeps the standard taxonomy.
fn reassign_error_to_rpc(
    error: WorkGraphError,
    binding_id: &WorkAttentionBindingId,
    mode: meerkat::WorkAttentionMode,
) -> JsonRpcError {
    match &error {
        WorkGraphError::InvalidInput(message)
            if message.contains("requires derived_from link authority") =>
        {
            let mode = serde_json::to_value(mode)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{mode:?}"));
            let detail = format!(
                "attention binding {binding_id} is in '{mode}' mode; meerkat 0.8.22 derives the \
                 derived_from link authority reassignment requires only for coordinate-mode \
                 bindings — pause the binding or recreate the goal with mode 'coordinate'",
            );
            JsonRpcError {
                code: WORKGRAPH_ERROR_CODE,
                message: format!("workgraph attention reassignment denied: {detail}"),
                data: Some(serde_json::json!({
                    "kind": "workgraph_error",
                    "detail": detail,
                })),
            }
        }
        _ => workgraph_error_to_rpc(error),
    }
}

/// Project an admission refusal onto the RPC wire taxonomy: occupancy
/// conflicts get the typed conflict code (the caller can free the target and
/// retry), check failures keep the standard workgraph error mapping, and a
/// failed cross-process sidecar lock fails CLOSED as a workgraph error — an
/// unserialized admission is exactly the race the lock exists to prevent.
fn admission_error_to_rpc(error: WorkGraphAdmissionError) -> JsonRpcError {
    match error {
        WorkGraphAdmissionError::Occupied { detail } => workgraph_conflict(detail),
        WorkGraphAdmissionError::Service(error) => workgraph_error_to_rpc(error),
        WorkGraphAdmissionError::Lock(detail) => {
            let detail = format!("workgraph admission lock failed: {detail}");
            JsonRpcError {
                code: WORKGRAPH_ERROR_CODE,
                message: detail.clone(),
                data: Some(serde_json::json!({
                    "kind": "workgraph_error",
                    "detail": detail,
                })),
            }
        }
    }
}

/// Dispatch one `meta/workgraph/*` request against `service`.
///
/// `trusted_principal` is the console surface's authenticated principal,
/// promoted into `goal/confirm` via `with_trusted_principal`; the unified
/// stdin surface passes `None` (the host process itself is the trusted
/// party there).
///
/// `admission` is the runtime-wide [`WorkGraphAdmission`]: its mob handle
/// (with the shared session store's member-binding metadata as the
/// roster-miss fallback) backs identity-target lowering and the
/// session↔identity alias resolution the duplicate-binding guards need, and
/// its gate serializes the guards' check-then-act windows — both RPC
/// surfaces (unified stdin + console) and the agent tool plane must pass the
/// SAME instance or concurrent creates race past the check.
/// Which RPC surface a workgraph call arrived on. The host stdin surface is
/// host-trusted but carries no wire principal; the console surface carries
/// the authenticated console principal (`None` when console auth is off),
/// which `goal/confirm` promotes into the trusted confirmation seam and
/// `attention/break_glass_reassign` requires for audit attribution.
#[derive(Debug, Clone, Copy)]
pub(crate) enum WorkgraphSurface<'a> {
    HostStdin,
    Console {
        authenticated_principal: Option<&'a str>,
    },
}

/// Params for `meta/workgraph/facts`.
///
/// Deliberately narrower than `events`: `namespace` is not accepted, because
/// the projection is scoped exactly like the fact tail (this service's realm
/// and default namespace). `deny_unknown_fields` makes that refusal explicit -
/// silently ignoring a supplied `namespace` would read to a caller as a filter
/// that works.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FactsParams {
    /// Exclusive lower bound; pass back
    /// `WorkGraphFactPage::next_after_seq`.
    #[serde(default)]
    after_seq: Option<i64>,
    /// Page size, clamped into `1..=MAX_FACT_POLL_LIMIT` by the projection.
    #[serde(default)]
    limit: Option<usize>,
}

pub(crate) async fn handle_workgraph_method(
    service: Option<&WorkGraphService>,
    admission: &WorkGraphAdmission,
    surface: WorkgraphSurface<'_>,
    method: &str,
    params: &Value,
) -> Result<Value, JsonRpcError> {
    if !is_workgraph_read_method(method) && !is_workgraph_mutating_method(method) {
        return Err(method_not_found());
    }
    // Console-only methods do not exist on the host stdin surface (mirrors
    // the console-methods-HTTP-only split in docs/api/rpc.mdx).
    if WORKGRAPH_CONSOLE_MUTATE_METHODS.contains(&method)
        && matches!(surface, WorkgraphSurface::HostStdin)
    {
        return Err(method_not_found());
    }
    let trusted_principal = match surface {
        WorkgraphSurface::HostStdin => None,
        WorkgraphSurface::Console {
            authenticated_principal,
        } => console_trusted_principal(authenticated_principal),
    };
    let Some(service) = service else {
        return Err(workgraph_unavailable_error());
    };
    let mob_id = &admission.mob_handle().definition().id;
    let object = params_object(params)?;
    // The service is realm-scoped at construction; a caller-supplied realm
    // would silently address foreign realm rows in the shared store file.
    if object.contains_key("realm_id") {
        return Err(invalid_params(
            "realm_id is not accepted; workgraph is scoped to this runtime's realm",
        ));
    }
    match method {
        "meta/workgraph/snapshot" => {
            let filter: WorkGraphSnapshotFilter = parse_request(object)?;
            let snapshot = service
                .snapshot(filter)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&snapshot))
        }
        "meta/workgraph/list" => {
            let filter: WorkItemFilter = parse_request(object)?;
            let items = service.list(filter).await.map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "items": items }))
        }
        "meta/workgraph/get" => {
            let request: WorkGraphIdParams = parse_request(object)?;
            let item = service
                .get(None, request.namespace, request.id)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "item": item }))
        }
        "meta/workgraph/ready" => {
            let filter: ReadyWorkFilter = parse_request(object)?;
            let items = service
                .ready(filter)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "items": items }))
        }
        "meta/workgraph/events" => {
            let filter: WorkGraphEventFilter = parse_request(object)?;
            let events = service
                .events(filter)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "events": events }))
        }
        "meta/workgraph/facts" => {
            let request: FactsParams = parse_request(object)?;
            let page = crate::workgraph_events::poll_workgraph_facts(
                service,
                request.after_seq,
                request
                    .limit
                    .unwrap_or(crate::workgraph_events::DEFAULT_FACT_POLL_LIMIT),
            )
            .await
            .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&page))
        }
        "meta/workgraph/attention/list" => {
            reject_non_default_namespace(service, &object)?;
            let mut object = object;
            normalize_attention_status_param(&mut object)?;
            let request: AttentionListRequest = parse_request(object)?;
            let result = service
                .list_attention(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&result))
        }
        "meta/workgraph/goal/status" => {
            let request: GoalStatusRequest = parse_request(object)?;
            let result = service
                .goal_status(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&result))
        }
        "meta/workgraph/create" => {
            let request: CreateWorkItemRequest = parse_request(object)?;
            let item = service
                .create(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "item": item }))
        }
        "meta/workgraph/update" => {
            let request: UpdateWorkItemRequest = parse_request(object)?;
            let item = service
                .update(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "item": item }))
        }
        "meta/workgraph/claim" => {
            let mut object = object;
            // The wire contract writes owner flat ({kind, id, display_name?});
            // upstream WorkOwner nests the key. Accept both shapes.
            if let Some(Value::Object(owner)) = object.get("owner")
                && !owner.contains_key("key")
                && owner.contains_key("kind")
            {
                let mut normalized = Map::new();
                normalized.insert(
                    "key".to_string(),
                    serde_json::json!({
                        "kind": owner.get("kind"),
                        "id": owner.get("id"),
                    }),
                );
                if let Some(display_name) = owner.get("display_name") {
                    normalized.insert("display_name".to_string(), display_name.clone());
                }
                object.insert("owner".to_string(), Value::Object(normalized));
            }
            let request: ClaimWorkItemRequest = parse_request(object)?;
            let item = service
                .claim(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "item": item }))
        }
        "meta/workgraph/release" => {
            let request: ReleaseWorkItemRequest = parse_request(object)?;
            let item = service
                .release(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "item": item }))
        }
        "meta/workgraph/close" => {
            let request: CloseWorkItemRequest = parse_request(object)?;
            if !matches!(
                request.status,
                WorkStatus::Completed | WorkStatus::Cancelled | WorkStatus::Failed
            ) {
                return Err(invalid_params(
                    "status must be one of: completed, cancelled, failed",
                ));
            }
            let item = service
                .close(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "item": item }))
        }
        "meta/workgraph/block" => {
            #[derive(serde::Deserialize)]
            struct BlockParams {
                id: WorkItemId,
                expected_revision: u64,
                #[serde(default)]
                namespace: Option<WorkNamespace>,
            }
            let request: BlockParams = parse_request(object)?;
            let item = service
                .block(
                    None,
                    request.namespace,
                    request.id,
                    request.expected_revision,
                )
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "item": item }))
        }
        "meta/workgraph/link" => {
            let request: LinkWorkItemsRequest = parse_request(object)?;
            let edge = service
                .link(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "edge": edge }))
        }
        "meta/workgraph/evidence/add" => {
            let request: AddEvidenceRequest = parse_request(object)?;
            let item = service
                .add_evidence(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "item": item }))
        }
        "meta/workgraph/policy/escalate" => {
            reject_non_default_namespace(service, &object)?;
            if object.contains_key("authority_projection") {
                return Err(invalid_params(
                    "authority_projection is not accepted; the authority witness is fetched \
                     server-side from binding_id",
                ));
            }
            #[derive(serde::Deserialize)]
            struct PolicyEscalateParams {
                id: WorkItemId,
                expected_revision: u64,
                completion_policy: meerkat::WorkCompletionPolicy,
                #[serde(default)]
                namespace: Option<WorkNamespace>,
            }
            let binding_id = parse_binding_id(&object)?;
            let mut object = object;
            object.remove("binding_id");
            let params: PolicyEscalateParams = parse_request(object)?;
            let projection =
                fetch_authority_projection(service, binding_id, params.namespace.clone()).await?;
            let item = service
                .escalate_policy(PolicyEscalateRequest {
                    id: params.id,
                    realm_id: None,
                    namespace: params.namespace,
                    expected_revision: params.expected_revision,
                    authority_projection: projection,
                    completion_policy: params.completion_policy,
                })
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(serde_json::json!({ "item": item }))
        }
        "meta/workgraph/goal/create" => {
            reject_non_default_namespace(service, &object)?;
            let mut object = object;
            let target_value = object
                .remove("target")
                .ok_or_else(|| invalid_params("target is required"))?;
            let target = resolve_goal_target(mob_id, &target_value)?;
            let target = admission
                .lower_member_session_target(target)
                .await
                .map_err(admission_error_to_rpc)?;
            crate::workgraph_realm::refuse_foreign_mob_target(&target, service)
                .map_err(workgraph_error_to_rpc)?;
            object.insert("target".to_string(), to_result_value(&target));
            let request: GoalCreateRequest = parse_request(object)?;
            let _permit = admission.acquire().await.map_err(admission_error_to_rpc)?;
            admission
                .check_target_free(
                    service,
                    request.namespace.clone(),
                    &request.target.to_attention_target(),
                    None,
                    "creating another goal for the same target",
                )
                .await
                .map_err(admission_error_to_rpc)?;
            let result = service
                .create_goal(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&result))
        }
        "meta/workgraph/goal/confirm" => {
            let binding_id = parse_binding_id(&object)?;
            let expected_revision = parse_expected_revision(&object)?;
            let namespace = parse_namespace(&object)?;
            let evidence = match object.get("evidence") {
                None | Some(Value::Null) => {
                    // Evidence-less confirm: derive the admissible evidence
                    // kind from the goal's completion policy.
                    let status = service
                        .goal_status(GoalStatusRequest {
                            binding_id: binding_id.clone(),
                            realm_id: None,
                            namespace: namespace.clone(),
                        })
                        .await
                        .map_err(workgraph_error_to_rpc)?;
                    default_confirm_evidence(&status.item.completion_policy, &binding_id)
                }
                Some(value) => parse_confirm_evidence(value)?,
            };
            let request = GoalConfirmRequest {
                binding_id,
                realm_id: None,
                namespace,
                expected_revision,
                evidence,
                principal: None,
                trusted_principal: None,
            }
            .with_trusted_principal(trusted_principal);
            let result = service
                .goal_confirm(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&result))
        }
        "meta/workgraph/goal/request_close" => {
            let request: GoalRequestCloseRequest = parse_request(object)?;
            let result = service
                .goal_request_close(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&result))
        }
        "meta/workgraph/attention/pause" => {
            reject_non_default_namespace(service, &object)?;
            let request: AttentionPauseRequest = parse_request(object)?;
            let result = service
                .pause_attention(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&result))
        }
        "meta/workgraph/attention/resume" => {
            reject_non_default_namespace(service, &object)?;
            let request: AttentionResumeRequest = parse_request(object)?;
            let _permit = admission.acquire().await.map_err(admission_error_to_rpc)?;
            admission
                .check_resume_target_free(service, request.namespace.clone(), &request.binding_id)
                .await
                .map_err(admission_error_to_rpc)?;
            let result = service
                .resume_attention(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&result))
        }
        "meta/workgraph/attention/reassign" => {
            reject_non_default_namespace(service, &object)?;
            if object.contains_key("authority_projection") {
                return Err(invalid_params(
                    "authority_projection is not accepted; the authority witness is fetched \
                     server-side from binding_id",
                ));
            }
            let binding_id = parse_binding_id(&object)?;
            let expected_revision = parse_expected_revision(&object)?;
            let namespace = parse_namespace(&object)?;
            let target_value = object
                .get("target")
                .ok_or_else(|| invalid_params("target is required"))?;
            let target = resolve_goal_target(mob_id, target_value)?;
            let target = admission
                .lower_member_session_target(target)
                .await
                .map_err(admission_error_to_rpc)?;
            crate::workgraph_realm::refuse_foreign_mob_target(&target, service)
                .map_err(workgraph_error_to_rpc)?;
            let _permit = admission.acquire().await.map_err(admission_error_to_rpc)?;
            admission
                .check_target_free(
                    service,
                    namespace.clone(),
                    &target.to_attention_target(),
                    Some(&binding_id),
                    "reassigning this binding onto the same target",
                )
                .await
                .map_err(admission_error_to_rpc)?;
            let projection =
                fetch_authority_projection(service, binding_id.clone(), namespace.clone()).await?;
            let binding_mode = projection.mode;
            let result = service
                .reassign_attention(AttentionReassignRequest {
                    binding_id: binding_id.clone(),
                    realm_id: None,
                    namespace,
                    expected_revision,
                    authority_projection: projection,
                    target,
                })
                .await
                .map_err(|error| reassign_error_to_rpc(error, &binding_id, binding_mode))?;
            Ok(to_result_value(&result))
        }
        "meta/workgraph/attention/prune" => {
            // Terminal-binding GC (upstream ask 24): deletes only
            // superseded/stopped binding rows; the event stream keeps the
            // audit history. Scope narrowing beyond this runtime's realm and
            // default namespace is the caller's `updated_before` bound.
            reject_non_default_namespace(service, &object)?;
            let request: AttentionPruneRequest = parse_request(object)?;
            let result = service
                .prune_terminal_attention(request)
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&result))
        }
        "meta/workgraph/attention/break_glass_reassign" => {
            // Upstream ask 23, doctrine-reframed: the ONE host-plane recovery
            // for a binding stuck on a wedged/retired agent with no
            // coordinator holding authority. Console surface only (the
            // HostStdin gate above); the principal is the authenticated
            // console principal — never a wire parameter — and a non-empty
            // reason is mandatory. Upstream records both in the workgraph
            // event stream and bypasses the authority witness while keeping
            // revision CAS, item non-terminality, and target occupancy.
            reject_non_default_namespace(service, &object)?;
            let WorkgraphSurface::Console {
                authenticated_principal,
            } = surface
            else {
                return Err(method_not_found());
            };
            let Some(principal) = authenticated_principal
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Err(JsonRpcError {
                    code: -32030,
                    message: "break-glass reassignment requires an authenticated console \
                              principal for audit attribution"
                        .to_string(),
                    data: Some(serde_json::json!({ "kind": "access_denied" })),
                });
            };
            if object.contains_key("principal") {
                return Err(invalid_params(
                    "principal is not accepted; the authenticated console principal is recorded",
                ));
            }
            if object.contains_key("authority_projection") {
                return Err(invalid_params(
                    "authority_projection is not accepted; break-glass bypasses the witness \
                     by design and is audited instead",
                ));
            }
            let binding_id = parse_binding_id(&object)?;
            let expected_revision = parse_expected_revision(&object)?;
            let namespace = parse_namespace(&object)?;
            let reason = object
                .get("reason")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| invalid_params("reason is required and must be non-empty"))?
                .to_string();
            let target_value = object
                .get("target")
                .ok_or_else(|| invalid_params("target is required"))?;
            let target = resolve_goal_target(mob_id, target_value)?;
            let target = admission
                .lower_member_session_target(target)
                .await
                .map_err(admission_error_to_rpc)?;
            crate::workgraph_realm::refuse_foreign_mob_target(&target, service)
                .map_err(workgraph_error_to_rpc)?;
            let _permit = admission.acquire().await.map_err(admission_error_to_rpc)?;
            admission
                .check_target_free(
                    service,
                    namespace.clone(),
                    &target.to_attention_target(),
                    Some(&binding_id),
                    "break-glass reassigning this binding onto the same target",
                )
                .await
                .map_err(admission_error_to_rpc)?;
            let result = service
                .break_glass_reassign_attention(BreakGlassAttentionReassignRequest {
                    binding_id,
                    realm_id: None,
                    namespace,
                    expected_revision,
                    target,
                    principal: principal.to_string(),
                    reason,
                })
                .await
                .map_err(workgraph_error_to_rpc)?;
            Ok(to_result_value(&result))
        }
        _ => Err(method_not_found()),
    }
}

/// Resolve the console surface's trusted principal for `goal/confirm`.
/// Invalid principal tokens degrade to `None` (the confirm then fails loudly
/// for policies that require a principal) rather than silently minting a
/// malformed owner key.
pub(crate) fn console_trusted_principal(
    authenticated_principal: Option<&str>,
) -> Option<WorkOwnerKey> {
    let principal = authenticated_principal
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    match WorkOwnerKey::principal(principal) {
        Ok(key) => Some(key),
        Err(error) => {
            tracing::warn!(
                target: "meta::workgraph",
                error = %error,
                "console principal could not be lowered to a workgraph owner key",
            );
            None
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use meerkat::WorkAttentionStatus;

    use super::*;

    #[test]
    fn method_predicates_partition_the_namespace() {
        for method in WORKGRAPH_READ_METHODS {
            assert!(is_workgraph_method(method));
            assert!(is_workgraph_read_method(method));
            assert!(!is_workgraph_mutating_method(method));
        }
        for method in WORKGRAPH_MUTATE_METHODS {
            assert!(is_workgraph_method(method));
            assert!(is_workgraph_mutating_method(method));
            assert!(!is_workgraph_read_method(method));
        }
        assert!(is_workgraph_method("meta/workgraph/bogus"));
        assert!(!is_workgraph_method("meta/memory/query"));
    }

    /// The fact projection is a READ. Every surface catalog and ABAC gate is
    /// driven off these arrays (`rpc.rs` advertises `WORKGRAPH_READ_METHODS`,
    /// `http_console.rs` maps `is_workgraph_read_method` to `workgraph.view`),
    /// so a fact page can never reach a caller who may not read WorkGraph.
    #[test]
    fn facts_is_a_read_method_on_every_surface() {
        assert!(WORKGRAPH_READ_METHODS.contains(&"meta/workgraph/facts"));
        assert!(is_workgraph_read_method("meta/workgraph/facts"));
        assert!(!is_workgraph_mutating_method("meta/workgraph/facts"));
        // Positive control: the predicate can say yes to a known mutator.
        assert!(is_workgraph_mutating_method("meta/workgraph/close"));
    }

    #[test]
    fn facts_params_refuse_a_silently_ignored_namespace() {
        let empty: FactsParams =
            serde_json::from_value(serde_json::json!({})).expect("empty params parse");
        assert_eq!(empty.after_seq, None);
        assert_eq!(empty.limit, None);

        let cursor: FactsParams = serde_json::from_value(serde_json::json!({
            "after_seq": 42,
            "limit": 8,
        }))
        .expect("cursor params parse");
        assert_eq!(cursor.after_seq, Some(42));
        assert_eq!(cursor.limit, Some(8));

        // A namespace filter does not exist here; accepting and dropping it
        // would read as a working scope narrowing.
        serde_json::from_value::<FactsParams>(serde_json::json!({ "namespace": "other" }))
            .expect_err("namespace must be refused, not ignored");
    }

    #[test]
    fn error_taxonomy_matches_wire_contract() {
        let conflict = workgraph_error_to_rpc(WorkGraphError::StaleRevision {
            id: WorkItemId::new("work_1").expect("id"),
            expected: 1,
            actual: 2,
        });
        assert_eq!(conflict.code, WORKGRAPH_CONFLICT_CODE);
        assert_eq!(
            conflict.data.as_ref().unwrap()["kind"],
            serde_json::json!("workgraph_conflict")
        );

        let conflict = workgraph_error_to_rpc(WorkGraphError::Conflict("busy".to_string()));
        assert_eq!(conflict.code, WORKGRAPH_CONFLICT_CODE);

        let params = workgraph_error_to_rpc(WorkGraphError::InvalidInput("bad".to_string()));
        assert_eq!(params.code, -32602);

        let other = workgraph_error_to_rpc(WorkGraphError::Store("io".to_string()));
        assert_eq!(other.code, WORKGRAPH_ERROR_CODE);
        assert_eq!(
            other.data.as_ref().unwrap()["kind"],
            serde_json::json!("workgraph_error")
        );
        assert!(
            other.data.as_ref().unwrap()["detail"]
                .as_str()
                .unwrap()
                .contains("io"),
            "full detail is disclosed (K2 posture)"
        );

        assert_eq!(
            workgraph_unavailable_error().code,
            WORKGRAPH_UNAVAILABLE_CODE
        );
    }

    /// Round-5 S2: `lowered_owner` — the spelling every serialized RESULT
    /// target carries — parses as an alias of `owner` with the same
    /// `owner_key` payload, so read-back targets round-trip into params.
    #[test]
    fn resolve_goal_target_accepts_the_lowered_owner_result_spelling() {
        let mob_id = meerkat_mob::MobId::from("round-trip-mob");
        let owner_key = WorkOwnerKey::principal("operator@example.test").expect("owner key");
        let owner_form = serde_json::json!({ "kind": "owner", "owner_key": owner_key });
        let lowered_form = serde_json::json!({ "kind": "lowered_owner", "owner_key": owner_key });

        let from_owner = resolve_goal_target(&mob_id, &owner_form).expect("owner form parses");
        let from_lowered =
            resolve_goal_target(&mob_id, &lowered_form).expect("lowered_owner form parses");
        assert_eq!(from_owner, from_lowered);

        // The serialized stored-target shape round-trips exactly: what a
        // result carries is what `to_attention_target` re-produces.
        assert_eq!(
            to_result_value(&from_lowered.to_attention_target()),
            lowered_form,
        );

        let error = resolve_goal_target(&mob_id, &serde_json::json!({ "kind": "mob" }))
            .expect_err("unknown kinds stay rejected");
        assert_eq!(error.code, -32602);
        assert!(error.message.contains("lowered_owner"), "{}", error.message);
    }

    #[test]
    fn console_principal_lowering_is_tolerant() {
        assert!(console_trusted_principal(None).is_none());
        assert!(console_trusted_principal(Some("   ")).is_none());
        let key = console_trusted_principal(Some("alice@example.test")).expect("principal key");
        assert_eq!(key.canonical(), "principal:alice@example.test");
    }

    /// Round-2 finding B: SDKs send the attention-list `status` filter as a
    /// bare string, upstream `WorkAttentionStatus` is internally tagged.
    /// Both spellings must parse; unknown strings are a params error.
    #[test]
    fn attention_status_param_accepts_both_wire_spellings() {
        for state in ["active", "paused", "superseded", "stopped"] {
            let mut object = Map::new();
            object.insert("status".to_string(), Value::String(state.to_string()));
            normalize_attention_status_param(&mut object).expect("bare string accepted");
            assert_eq!(object["status"], serde_json::json!({ "state": state }));
            let request: AttentionListRequest =
                parse_request(object).expect("normalized status parses upstream");
            assert!(request.status.is_some(), "{state}");
        }

        let mut object = Map::new();
        object.insert(
            "status".to_string(),
            serde_json::json!({ "state": "paused" }),
        );
        normalize_attention_status_param(&mut object).expect("tagged form passes through");
        let request: AttentionListRequest = parse_request(object).expect("tagged form parses");
        assert!(matches!(
            request.status,
            Some(WorkAttentionStatus::Paused { until: None })
        ));

        let mut object = Map::new();
        object.insert("status".to_string(), Value::String("bogus".to_string()));
        let error =
            normalize_attention_status_param(&mut object).expect_err("unknown string rejected");
        assert_eq!(error.code, -32602);
        assert!(error.message.contains("active"), "{}", error.message);
    }

    /// Adversarial finding F12: a witness that goes stale between fetch and
    /// use (the item moved underneath) must surface as the retryable -32042
    /// conflict, not the generic -32000. Drives the real service so the
    /// upstream `InvalidTransition` spelling the message-guard matches on
    /// stays pinned.
    #[tokio::test]
    async fn stale_attention_witness_maps_to_conflict() {
        use meerkat::{
            AttentionProjectionRequest, GoalAttentionTarget, GoalCreateRequest,
            PolicyEscalateRequest, UpdateWorkItemRequest, WorkCompletionPolicy,
        };

        let service = crate::workgraph_wiring::ephemeral_workgraph_service("stale-realm");
        let goal = service
            .create_goal(GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: "goes stale".to_string(),
                description: None,
                target: GoalAttentionTarget::Owner {
                    owner_key: WorkOwnerKey::principal("operator@example.test").expect("key"),
                },
                mode: Default::default(),
                completion_policy: Default::default(),
                // meerkat 0.8.22 promoted the item-shaping fields of
                // `CreateWorkItemRequest` onto `GoalCreateRequest`. Eight of
                // the ten already existed on `CreateWorkItemRequest` in
                // 0.8.21, where `create_goal` filled them from
                // `..CreateWorkItemRequest::default()`; the two join policies
                // are new in 0.8.22 and so had no prior value to preserve.
                // The values below therefore reproduce the pre-port item
                // exactly. Both join policies are inert here in any case:
                // this fixture creates no `parent` edges, so no child can
                // fail or cancel into this goal.
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: Default::default(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
                delegated_authority: Default::default(),
                projection_policy: Default::default(),
            })
            .await
            .expect("create goal");
        let stale_witness = service
            .attention_projection(AttentionProjectionRequest {
                binding_id: goal.attention.binding_id.clone(),
                realm_id: None,
                namespace: None,
            })
            .await
            .expect("attention projection")
            .projection;
        // Bump the item revision underneath the witness.
        let bumped = service
            .update(UpdateWorkItemRequest {
                id: goal.item.id.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: goal.item.revision,
                title: Some("moved underneath".to_string()),
                description: None,
                priority: None,
                completion_policy: None,
                labels: None,
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
            })
            .await
            .expect("bump item revision");
        let error = service
            .escalate_policy(PolicyEscalateRequest {
                id: goal.item.id.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: bumped.revision,
                authority_projection: stale_witness,
                completion_policy: WorkCompletionPolicy::HostConfirmed,
            })
            .await
            .expect_err("a stale witness must be rejected");
        assert!(
            matches!(&error, WorkGraphError::InvalidTransition(message)
                if message.starts_with(STALE_ATTENTION_WITNESS_PREFIX)),
            "pins the upstream stale-witness spelling: {error}"
        );

        let rpc_error = workgraph_error_to_rpc(error);
        assert_eq!(rpc_error.code, WORKGRAPH_CONFLICT_CODE, "{rpc_error:?}");
        assert_eq!(
            rpc_error.data.as_ref().unwrap()["kind"],
            serde_json::json!("workgraph_conflict")
        );
        assert!(
            rpc_error.data.as_ref().unwrap()["detail"]
                .as_str()
                .unwrap()
                .contains(STALE_ATTENTION_WITNESS_PREFIX),
            "{rpc_error:?}"
        );
    }
}
