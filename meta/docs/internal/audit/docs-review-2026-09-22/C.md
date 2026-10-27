# C: HTTP, JSON-RPC, SSE, authentication, access and events

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## C-001: gating/evaluate documents a request and result schema the dispatcher does not implement

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:644-665`**

```text
| `action_id` | `string` | yes | Unique action identifier |
| `actor_id` | `string` | yes | Agent or user performing the action |
| `action_type` | `string` | yes | Action category (e.g. `"delivery.send"`) |
| `context` | `object` | no | Additional context for risk assessment |
```

A request constructed from the parameter table fails with -32602, and a response decoder built from the example looks for nonexistent fields.

**`meerkat-mobkit/src/rpc/gating_methods.rs:86-116`**

```text
let action = object
        .get("action")
```

The parser requires non-empty action and actor_id plus risk_tier; it never reads the documented action_id, action_type or context. The risk-tier parser at 261-270 accepts r0/r1/r2/r3.

**`meerkat-mobkit/src/runtime.rs:931-941`**

```text
pub struct GatingEvaluateResult {
    pub action_id: String,
    pub action: String,
    pub actor_id: String,
    pub risk_tier: GatingRiskTier,
    pub outcome: GatingOutcome,
```

The result has generated action_id, action, actor_id, r0-r3 risk_tier, outcome, pending_id and fallback_reason, not the example's safe_dispatch/explanation/recommended_handling fields.

**`meerkat-mobkit/tests/gating_policy.rs:266-283`**

```text
"action":"publish_release",
                "actor_id":"alice",
                "risk_tier":"r3",
```

An existing contract test uses the actual schema.

### Independent adjudication

The documented evaluate request fails the real parameter parser, and its response is not the serialized runtime result. I traced the unified dispatcher rather than inferring the wire contract from type names. Evaluation takes a caller-supplied risk tier and applies policy; it does not infer a safe_dispatch tier from context. This is the stdio/module operational method, not an invitation to add an HTTP evaluate arm.

**`meerkat-mobkit/src/rpc.rs:2995-3012`**

```text
"mobkit/gating/evaluate" => match parse_gating_evaluate_params(&request.params) {
```

Dispatch invokes the checked parser, serializes GatingEvaluateResult, and maps parser failures to -32602.

**`meerkat-mobkit/src/rpc/gating_methods.rs:87-116`**

```text
.get("action")
```

The required strings are action and actor_id plus a parsed risk_tier. action_id/action_type/context do not substitute for these fields.

**`meerkat-mobkit/src/runtime.rs:921-940`**

```text
pub enum GatingOutcome {
    Allowed,
    AllowedWithAudit,
    PendingApproval,
    SafeDraft,
}
```

Snake-case outcomes and GatingEvaluateResult fields, including action_id/action/actor_id/risk_tier/outcome/pending_id/fallback_reason, contradict the displayed response.

**`meerkat-mobkit/tests/gating_policy.rs:270-283`**

```text
"risk_tier":"r3",
```

The existing dispatched contract test supplies action, actor_id and r3; it is not merely a direct type-construction test.

**Required correction:** Replace the evaluate request table with required action, actor_id and risk_tier (r0/r1/r2/r3), and optional rationale, requested_approver, approval_recipient, approval_channel, approval_timeout_ms, entity and topic. Explain policy evaluation of the supplied tier. Show the actual result fields and allowed/allowed_with_audit/pending_approval/safe_draft outcomes, with nullable pending_id/fallback_reason. Do not imply HTTP supports this method.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Replaced the nonexistent evaluate schema with action/actor_id/risk_tier and all seven optional controls; documented supplied-tier policy evaluation, the gateway's configured-tier precedence, actual result fields, nullable fields and outcome vocabulary, and stdio-only availability.

**Validation:** Compared rpc/gating_methods.rs, runtime.rs::GatingEvaluateResult, runtime/gating.rs and rpc_gateway.rs::apply_gateway_runtime_config_to_request. Read-only Python checked the table inventory and exact response keys against the public result struct.

**Final review: pass.** After the B-007 residual correction, the request table correctly distinguishes matching rpc_gateway SDK/stdio policy from the ordinary downstream parser: matching policy supplies an omitted risk_tier and overrides a supplied one; otherwise the caller must provide it. The other parameter controls, exact result keys/enum values and SDK/stdin-only availability remain correct.

**`docs/api/rpc.mdx:674-710`**

```text
For matching `rpc_gateway` SDK/stdio policy, an omitted tier is filled and a supplied tier is overridden by the configured action risk tier. Otherwise, the caller must provide it
```

The final introduction and Required column both qualify the caller's risk_tier obligation. action and actor_id remain required. The unchanged response has action_id/action/actor_id/risk_tier/outcome/pending_id/fallback_reason.

**`meerkat-mobkit/src/rpc/gating_methods.rs:91-187`**

```text
let risk_tier = parse_gating_risk_tier(risk_tier)?;
```

Read the complete parameter parser and independently compared every documented field. rpc.rs:2995-3013 dispatches this parser and directly serializes the result.

**`meerkat-mobkit/src/runtime.rs:921-940`**

```text
pub struct GatingEvaluateResult {
```

An independent Python assertion compared the example's seven keys exactly with this struct. Its Option fields serialize as null. rpc_gateway.rs:6707 replaces risk_tier with a configured action tier.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6679-6707`**

```text
params.insert("risk_tier".to_string(), Value::String(risk_tier.clone()));
```

For an evaluate request whose trimmed action matches action_risk_tiers, insertion is unconditional: absence is filled and an existing value is replaced. The warning is conditional on a conflicting supplied string, not insertion. Without a matching policy, the ordinary parser still requires a string tier.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:13659-13660`**

```text
apply_gateway_runtime_config_to_request(&request_line, &gateway_options.gating);
```

The live SDK/stdin dispatch loop invokes the rewrite before dispatch, establishing the exception without claiming that unrelated HTTP/library calls receive the same rewriting.

## C-002: gating/decide uses the wrong approver parameter and hides surface-specific decision handling

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:666-677`**

```text
| `decision` | `string` | yes | `"approve"` or `"deny"` |
| `decided_by` | `string` | yes | Who made the decision |
```

The documented stdio example cannot approve or deny anything; consumers also miss escalation and can incorrectly assume an HTTP caller-selected approver is authoritative.

**`meerkat-mobkit/src/rpc/gating_methods.rs:203-230`**

```text
let approver_id = object
        .get("approver_id")
```

Stdio requires approver_id, not decided_by, and accepts optional reason.

**`meerkat-mobkit/src/rpc/gating_methods.rs:273-281`**

```text
"approve" => Ok(GatingDecision::Approve),
        "reject" => Ok(GatingDecision::Reject),
        "escalate" => Ok(GatingDecision::Escalate),
```

The standard parser rejects deny; the complete supported set includes reject and escalate.

**`meerkat-mobkit/src/http_console.rs:5356-5373`**

```text
return Ok(principal.to_string());
```

resolve_gating_approver_id takes an authenticated HTTP principal before considering params.approver_id; without one it requires approver_id. The HTTP decision arm at 7409-7413 additionally accepts deny as an alias for reject.

### Independent adjudication

Both the wrong parameter name and the cross-surface decision distinction are real. The stdio parser requires approver_id and supports reject/escalate, while HTTP gives the resolved principal precedence and additionally accepts deny. Narrow the original 'open-console calls require approver_id' wording: an auth-optional console can still resolve a voluntarily supplied token, so the condition is absence of a resolved principal, not simply require_app_auth=false.

**`meerkat-mobkit/src/rpc/gating_methods.rs:203-230`**

```text
.get("approver_id")
```

The dispatched stdio parser requires a non-empty approver_id; decided_by is not read.

**`meerkat-mobkit/src/rpc/gating_methods.rs:273-281`**

```text
"escalate" => Ok(GatingDecision::Escalate),
```

The actual vocabulary is approve/reject/escalate, not approve/deny.

**`meerkat-mobkit/src/http_console.rs:5356-5373`**

```text
return Ok(principal.to_string());
```

HTTP uses a non-empty authenticated principal first and only otherwise requires params.approver_id.

**`meerkat-mobkit/src/http_console.rs:7405-7438`**

```text
"reject" | "deny" => GatingDecision::Reject,
```

The HTTP arm accepts deny as a compatibility alias and also reads optional reason.

**`meerkat-mobkit/src/http_console.rs:11395-11412`**

```text
Ok("admin@example.com".to_string()),
```

Existing tests explicitly reject forged browser attribution in favor of the principal, and require a parameter when there is no principal.

**Required correction:** Document pending_id, approve/reject/escalate, approver_id, and optional reason. Explain that stdio requires approver_id; HTTP requires it only without a resolved principal, otherwise uses that principal regardless of the supplied field. Document HTTP's deny alias. In the current v0.5.0 HTTP contract make approver_id conditionally required. Do not change older versioned contracts.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`, `docs/rct/console-rest-sse-contract-v0.5.0.json`.

Documented approve/reject/escalate, approver_id and optional reason; HTTP principal precedence, conditional approver_id when no principal resolves, and HTTP-only deny alias. Updated only the canonical v0.5 request.

**Validation:** Checked gating_methods.rs parser and http_console.rs approver resolver/decision arm; JSON assertions verify approver_id is not unconditionally required and the conditional explanation is present.

**Final review: pass.** The reference and canonical v0.5 request agree on approver_id, reason, approve/reject/escalate, HTTP-only deny, and principal precedence. Crucially, the condition is absence of a resolved principal, not merely optional authentication.

**`docs/api/rpc.mdx:711-727`**

```text
HTTP derives the approver from the resolved principal, ignoring any supplied
```

The adjacent table states stdio-required/HTTP-conditional approver_id and the text explicitly covers valid volunteered tokens.

**`docs/rct/console-rest-sse-contract-v0.5.0.json:747-772`**

```text
"conditional_required_fields": {
```

Independent JSON assertions verified pending_id/decision are required, approver_id is conditional, and all four HTTP decision spellings are listed.

**`meerkat-mobkit/src/http_console.rs:5356-5373`**

```text
return Ok(principal.to_string());
```

The HTTP resolver prefers the principal. HTTP decision dispatch at 7408-7423 admits deny and reason; the stdio parser at gating_methods.rs:203-242,273-281 requires approver_id and excludes deny.

## C-003: routing/resolve uses destination and sink_adapter instead of the actual wire fields

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:789-808`**

```text
| `destination` | `string` | yes | Logical destination identifier |
```

The request is refused, and copying the displayed resolution into delivery/send produces an invalid-resolution error.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:180-200`**

```text
let recipient = object
        .get("recipient")
```

The required target field is recipient. The parser also accepts retry_max, backoff_ms and rate_limit_per_minute.

**`meerkat-mobkit/src/runtime.rs:720-730`**

```text
pub struct RoutingResolution {
    pub route_id: String,
    pub recipient: String,
    pub channel: String,
    pub sink: String,
    pub target_module: String,
    pub retry_max: u32,
    pub backoff_ms: u64,
    pub rate_limit_per_minute: u32,
}
```

The documented response instead uses sink_adapter and omits fields required when the resolution is deserialized for delivery/send.

### Independent adjudication

The actual routing/resolve dispatcher calls a recipient-based parser and serializes RoutingResolution directly. No destination or sink_adapter translation exists in this path. sink_adapter does exist on DeliveryRecord, but that separate type cannot rescue the routing response example. All fields of RoutingResolution are required when its value is later deserialized for delivery/send.

**`meerkat-mobkit/src/rpc.rs:2408-2429`**

```text
let resolve_result = match parse_routing_resolve_params(&request.params) {
```

The parser and direct result serialization govern the public method.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:180-260`**

```text
.get("recipient")
```

recipient is required; channel, retry_max, backoff_ms and rate_limit_per_minute are optional.

**`meerkat-mobkit/src/runtime.rs:720-736`**

```text
pub sink: String,
```

RoutingResolution carries route_id, recipient, channel, sink, target_module and all three delivery controls; DeliverySendRequest embeds that exact type.

**`meerkat-mobkit/tests/routing_delivery.rs:270-298`**

```text
assert_eq!(resolved["result"]["sink"], json!("email"));
```

The existing test resolves by recipient and passes the complete result into delivery/send.

**Required correction:** Replace destination with recipient and sink_adapter with sink. Add optional retry_max/backoff_ms/rate_limit_per_minute request overrides. Show a complete server-issued RoutingResolution, retaining the notification channel default; tell clients to pass the returned resolution rather than construct route_id values from a presumed member/channel format.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Changed routing resolve to recipient/sink, added retry/backoff/rate overrides, and supplied every RoutingResolution field with a server-issued route-ID example. Instructed callers to preserve the actual returned resolution.

**Validation:** Checked routing_delivery_methods.rs, runtime.rs::RoutingResolution and runtime/routing.rs route-000001 formatting; Python compared response keys exactly with the struct fields.

**Final review: pass.** recipient, sink and all delivery controls now match the real request/result. The response example is complete, and the preservation warning correctly distinguishes an issued route_id from an authored route_key.

**`docs/api/rpc.mdx:850-887`**

```text
Pass the complete returned resolution unchanged
```

The example has all eight RoutingResolution keys; the request table uses recipient and the three optional delivery overrides.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:180-246`**

```text
.get("recipient")
```

Read the complete parser; rpc.rs:2408-2421 directly serializes the runtime resolution. runtime.rs:720-728 supplies the exact eight response fields, and runtime/routing.rs:360 issues route-NNNNNN IDs.

## C-004: The route mutation reference omits recipient and uses the wrong key/backend names

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:820-831`**

```text
| `route_id` | `string` | yes | Unique route identifier |
| `channel` | `string` | yes | Delivery channel |
| `sink_adapter` | `string` | yes | Physical delivery backend |
| `target_module` | `string` | yes | Module that handles delivery |
```

Neither adding a route using the table nor deleting one by the documented route_id succeeds.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:264-309`**

```text
let route_value = object.get("route").unwrap_or(params);
```

The handler accepts a top-level route or a route wrapper, requires route_key, recipient, sink and target_module, and makes channel optional.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:351-365`**

```text
let route_key = object
        .get("route_key")
```

routes/delete also requires route_key; that necessary parameter is missing from its one-line description.

**`meerkat-mobkit/tests/routing_delivery.rs:125-125`**

```text
"route_key":"vip-route","recipient":"vip@example.com","channel":"notification","sink":"sms","target_module":"delivery"
```

The contract test demonstrates the actual add-route field names.

### Independent adjudication

The add-route parser accepts either a top-level route or a route wrapper, but both use route_key/recipient/sink/target_module and optional channel. The stdio dispatcher calls this parser and routes/delete independently requires route_key. The current table cannot produce a valid request; the bare delete description supplies no usable key.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:264-309`**

```text
let route_value = object.get("route").unwrap_or(params);
```

Both supported layouts flow to the same route_key, recipient, sink and target_module parser. channel is optional.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:350-365`**

```text
.get("route_key")
```

Deletion uses a top-level non-empty route_key, not the resolution's route_id.

**`meerkat-mobkit/src/rpc.rs:2458-2498`**

```text
let delete_result = match parse_routing_route_delete_params(&request.params) {
```

Dispatch preserves these names without a documentation-compatible alias.

**`meerkat-mobkit/tests/routing_delivery.rs:125-125`**

```text
"route_key":"vip-route","recipient":"vip@example.com","channel":"notification","sink":"sms","target_module":"delivery"
```

The existing RPC test supplies the real add-route shape.

**Required correction:** Replace the add table with required route_key, recipient, sink, target_module; optional channel, retry_max, backoff_ms, rate_limit_per_minute. Explain the optional route wrapper. Add routes/delete params {route_key}. Do not conflate authored route_key with an issued RoutingResolution.route_id.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Corrected authored routes to route_key/recipient/sink/target_module, made channel optional, documented optional delivery controls and params.route wrapping, and added the required deletion route_key.

**Validation:** Checked parse_routing_route_add_params and parse_routing_route_delete_params; Python verified the full add-route parameter inventory.

**Final review: pass.** The add/delete schemas now use the correct authored key and recipient/backend fields, preserve optional channel/controls, and describe both accepted add layouts. There is no remaining route_id/sink_adapter substitution in these parameter tables.

**`docs/api/rpc.mdx:885-910`**

```text
Fields may be directly in `params` or wrapped as
```

The add table and delete example consistently use route_key; recipient, sink and target_module are required.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:260-365`**

```text
let route_value = object.get("route").unwrap_or(params);
```

Both layouts reach the same parser, while deletion independently requires top-level route_key. rpc.rs:2458-2470 actually dispatches this parser.

## C-005: delivery/send incorrectly requires an idempotency key and restricts payloads to objects

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:903-905`**

```text
| `payload` | `object` | yes | Message payload |
| `idempotency_key` | `string` | yes | Key for idempotent delivery |
```

Clients unnecessarily reject valid payloads and implement a required field the wire protocol does not require.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:380-416`**

```text
let payload = object
        .get("payload")
        .cloned()
        .ok_or(RoutingDeliveryParamsError::PayloadRequired)?;
    let idempotency_key = match object.get("idempotency_key") {
        None => None,
```

Any JSON Value is accepted as payload and the key is explicitly optional. A supplied key must be a non-empty string.

**`meerkat-mobkit/src/runtime.rs:732-737`**

```text
pub payload: Value,
    #[serde(default)]
    pub idempotency_key: Option<String>,
```

The public request type confirms both facts.

### Independent adjudication

The public request type and the actual stdio parser both make the key optional. The parser clones any present JSON payload without requiring an object; runtime forwarding does not justify the documentation's object-only restriction. Existing end-to-end routing coverage successfully sends with no key.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:380-410`**

```text
let idempotency_key = match object.get("idempotency_key") {
        None => None,
```

A missing key is explicitly accepted; a provided one must be a non-empty string. Payload is cloned as a Value.

**`meerkat-mobkit/src/runtime.rs:732-736`**

```text
pub payload: Value,
```

The public wire type permits any JSON value and uses Option<String> for the key.

**`meerkat-mobkit/tests/routing_delivery.rs:278-298`**

```text
"payload": {"message": "hello"}
```

This actual delivery/send request omits idempotency_key and asserts a sent result.

**Required correction:** Describe payload as a required JSON value, not necessarily an object. Mark idempotency_key optional, with non-empty-string validation when supplied; callers seeking keyed replay protection must supply it.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Allowed any required JSON payload and made the delivery idempotency key optional, non-empty after trimming when supplied, and necessary for keyed replay protection.

**Validation:** Checked parse_delivery_send_params and DeliverySendRequest; no runtime behavior changed.

**Final review: pass.** The payload is now any JSON value and idempotency_key is optional, with correct nonempty-after-trimming validation when supplied. The correction does not confuse this operational delivery API with console/send's mandatory key.

**`docs/api/rpc.mdx:974-982`**

```text
| `payload` | JSON value | yes | Message payload; need not be an object |
```

The following row makes the key optional and qualifies keyed replay protection.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:367-405`**

```text
let payload = object
```

The parser clones the required JSON Value without object restriction and accepts None for the key. runtime.rs:732-736 agrees.

## C-006: The roster section promises HTTP passthrough for methods only served by stdio

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:305-308`**

```text
The roster family is served by the SDK stdio surface and, as runtime
passthrough, by `POST /console/rpc`.
```

An HTTP integrator calls the documented roster reconciliation/discovery methods and gets -32601 despite the promised passthrough.

**`meerkat-mobkit/src/http_console.rs:8890-8941`**

```text
_ => response_value(
            response_id,
            None,
            Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
```

The HTTP dispatcher has explicit arms and no general unified-runtime passthrough. It contains no rediscover, reconcile_identity, or reconcile arm; its method list at 5486-5620 omits those three.

**`meerkat-mobkit/src/rpc.rs:3334-3335`**

```text
"mobkit/rediscover" => mob_methods::handle_rediscover(runtime, response_id).await,
```

rediscover is implemented by the unified stdio dispatcher, while reconcile_identity is separately handled there at 4760 onward.

**`meerkat-mobkit/src/http_console.rs:8033-8038`**

```text
"mobkit/reconcile_edges" => {
```

reconcile_edges does have an HTTP arm, demonstrating that the distinction is method-specific, not that all reconciliation is absent.

### Independent adjudication

I followed /console/rpc through both runtime-attached and aggregator-only dispatch, including the separately dispatched member-declaration and WorkGraph families. Neither is a general stdio passthrough. rediscover, reconcile_identity and module reconcile have no HTTP branch; their methods are present on the unified dispatcher. reconcile_edges really is exposed on HTTP, so an all-or-nothing reconciliation claim would also be wrong.

**`meerkat-mobkit/src/http_console.rs:658-674`**

```text
let response_value = Box::pin(handle_console_runtime_rpc_with_visibility(
```

The route selects its own aggregator/runtime handlers rather than handle_unified_rpc_json.

**`meerkat-mobkit/src/http_console.rs:8931-8939`**

```text
code: -32601,
```

Unknown runtime HTTP methods terminate as Method not found. Read-only source search found zero literals for the three disputed methods anywhere in http_console.rs.

**`meerkat-mobkit/src/rpc.rs:3334-3335`**

```text
"mobkit/rediscover" => mob_methods::handle_rediscover(runtime, response_id).await,
```

Unified/stdin dispatch does implement rediscover; reconcile and reconcile_identity have distinct arms at 2206 and 4760.

**`meerkat-mobkit/src/http_console.rs:8033-8033`**

```text
"mobkit/reconcile_edges" => {
```

The HTTP distinction is per method, not per broad family.

**Required correction:** Remove the general runtime-passthrough promise from the introduction and roster section. Mark rediscover, reconcile_identity and module reconcile as SDK/stdin-only; retain HTTP support for implemented member operations and reconcile_edges, subject to runtime capabilities. Do not implement new routes as a documentation fix.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Removed the HTTP runtime-passthrough promise, retained implemented member operations/reconcile_edges, and marked rediscover, reconcile_identity and module reconcile SDK/stdin-only.

**Validation:** Compared independent http_console.rs dispatch and rpc.rs arms; the corrected introduction, roster explanation and method rows agree on the surface split.

**Final review: pass.** The blanket HTTP passthrough promise is removed in both repeated locations. Method rows and overview now correctly reserve rediscover, reconcile_identity and module reconcile for SDK/stdin while retaining HTTP reconcile_edges.

**`docs/api/rpc.mdx:7-7`**

```text
The HTTP dispatcher is not a general stdio passthrough
```

The roster section at 309-350 repeats the precise per-method surface split instead of contradicting the introduction.

**`meerkat-mobkit/src/http_console.rs:8033-8038`**

```text
"mobkit/reconcile_edges" => {
```

Read-only exact-string checks found no HTTP literals for rediscover, reconcile_identity or reconcile, while all exist in rpc.rs. The HTTP dispatcher has its own terminal method-not-found branch.

## C-007: The 30-second read deadline is described as an HTTP console guarantee but only wraps stdio dispatch

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:305-308`**

```text
Roster reads cross the mob actor's
sequential command loop and are bounded by the `-32017` read budget.
```

Operators expect MOBKIT_CONSOLE_READ_TIMEOUT_SECS to bound HTTP calls, but that knob cannot provide the claimed HTTP guarantee.

**`meerkat-mobkit/src/rpc.rs:3271-3283`**

```text
"mobkit/list_members" => {
            with_read_deadline(
```

The unified/stdin dispatcher applies the private with_read_deadline helper; its environment-controlled budget and -32017 response are defined at 1700-1777.

**`meerkat-mobkit/src/http_console.rs:6268-6272`**

```text
"mobkit/list_members" => {
            let handle = runtime.handle();
            let entries = handle.list_members_including_retiring().await;
```

The independent HTTP arm directly awaits the actor read. Neither console_rpc_handler nor its runtime dispatcher wraps it with that deadline, and http_console.rs does not invoke with_read_deadline.

### Independent adjudication

The 30-second helper belongs to the unified dispatcher. The stock HTTP route directly awaits its separate roster arms and does not call the helper. I also checked the other HTTP timeout occurrence: it is a five-second voice-readiness operation, not a blanket request/read timeout. The name 'console_read_timeout' alone does not establish an HTTP guarantee.

**`meerkat-mobkit/src/rpc.rs:1700-1723`**

```text
const CONSOLE_READ_BUDGET: Duration = Duration::from_secs(30);
```

The env-based override is clamped to 1..3600 in this private helper.

**`meerkat-mobkit/src/rpc.rs:3271-3282`**

```text
with_read_deadline(
```

The unified list_members arm wraps its future.

**`meerkat-mobkit/src/http_console.rs:6268-6271`**

```text
let entries = handle.list_members_including_retiring().await;
```

The HTTP arm awaits directly; its enclosing router/dispatcher does not apply this budget.

**`meerkat-mobkit/src/rpc.rs:6084-6091`**

```text
async fn a_blocked_read_arm_returns_the_typed_timeout_instead_of_hanging() {
```

The existing paused-time regression test exercises with_read_deadline itself, not the HTTP route.

**Required correction:** Scope the -32017 error-table entry and roster read-budget paragraph to the covered unified/stdin dispatcher arms. Explicitly say the independent stock HTTP dispatcher does not apply that wrapper; host/client HTTP timeouts are a separate concern. Do not describe every roster operation as deadline-wrapped.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Scoped -32017 and the 30-second/env-controlled budget to enumerated unified/stdin read arms, not every roster operation; explicitly excluded the independent HTTP dispatcher and separated host/client HTTP timeouts.

**Validation:** Checked rpc.rs::with_read_deadline coverage against direct HTTP reads; both repeated prose locations were corrected.

**Final review: pass.** Both deadline descriptions are now scoped to the covered unified/stdin arms, enumerate the correct eight production wrappers, and explicitly exclude the independent HTTP dispatcher. The default and environment clamp remain accurate.

**`docs/api/rpc.mdx:138-138`**

```text
The independent HTTP console dispatcher does not apply this wrapper.
```

The roster overview at 316-318 also says not every operation is wrapped and distinguishes host/client HTTP timeouts.

**`meerkat-mobkit/src/rpc.rs:1700-1723`**

```text
const CONSOLE_READ_BUDGET: Duration = Duration::from_secs(30);
```

The helper clamps to 1..3600. An exact call inventory matched the documented arms; http_console.rs contains no with_read_deadline call and directly awaits list_members at 6268-6270.

## C-008: ensure_member spawn errors are not uniformly -32602 across the documented surfaces

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:362-364`**

```text
Spawn refusals come back as `-32602` with the meerkat error in the message,
```

Cross-surface clients classify HTTP worker spawn failures incorrectly and may miss their error-handling branch.

**`meerkat-mobkit/src/rpc/mob_methods.rs:1001-1011`**

```text
code: -32602,
                        message: format!("ensure_member failed: {err}"),
```

This claim is true for stdio.

**`meerkat-mobkit/src/http_console.rs:7674-7675`**

```text
Err(err) => internal_error(response_id, format!("ensure_member failed: {err}")),
```

The worker-plane HTTP arm uses internal_error, the console's -32000 error helper, not -32602. Identity-plane materialization can instead return the already documented broken receipt.

**`meerkat-mobkit/src/http_console.rs:4064-4071`**

```text
code: -32000,
```

This is the concrete numeric value emitted by internal_error.

### Independent adjudication

The existing prose already distinguishes identity-plane broken receipts, but fails to distinguish the HTTP worker-plane failure from the stdio worker-plane error. The actual error helper proves -32000 on HTTP; there is no generic conversion to -32602 before serialization.

**`meerkat-mobkit/src/rpc/mob_methods.rs:1002-1010`**

```text
code: -32602,
```

Stdio ensure_member spawn failures put the underlying error in the message with -32602.

**`meerkat-mobkit/src/http_console.rs:7674-7674`**

```text
Err(err) => internal_error(response_id, format!("ensure_member failed: {err}")),
```

The HTTP worker arm uses the other helper.

**`meerkat-mobkit/src/http_console.rs:4064-4070`**

```text
code: -32000,
```

internal_error returns -32000 with structured error/detail data.

**Required correction:** Qualify the spawn-error sentence: stdio worker ensure failures use -32602; HTTP worker ensure failures use -32000. Keep the existing identity-first materialization/broken-receipt caveat and avoid changing runtime error codes.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Separated stdio worker spawn refusal -32602 from HTTP worker refusal -32000, while retaining the identity-first successful broken-receipt caveat.

**Validation:** Compared rpc/mob_methods.rs ensure handling with the HTTP worker arm and internal_error helper.

**Final review: pass.** Worker spawn errors now distinguish stdio -32602 from HTTP -32000. The existing successful identity-plane broken-receipt caveat remains instead of being overwritten by a universal error claim.

**`docs/api/rpc.mdx:378-390`**

```text
Worker-plane spawn refusals use `-32602` on stdio and `-32000` on HTTP,
```

The adjoining explanation retains identity-first materialization behavior.

**`meerkat-mobkit/src/rpc/mob_methods.rs:1002-1010`**

```text
code: -32602,
```

The HTTP worker arm instead calls internal_error at http_console.rs:7674; that helper explicitly emits -32000 at 4064-4070.

## C-009: memory/query omits its implemented text filter while describing only exact filtering

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:715-725`**

```text
Query stored assertions and conflict signals by exact filters.
```

Readers conclude that substring text search is unsupported and cannot use the existing query parameter from the reference.

**`meerkat-mobkit/src/rpc/memory_methods.rs:626-641`**

```text
let query = match object.get("query") {
```

The public RPC parser accepts the query string in addition to entity/topic/store.

**`meerkat-mobkit/src/runtime/memory.rs:374-399`**

```text
assertion.entity.contains(needle.as_str())
                    || assertion.topic.contains(needle.as_str())
```

The runtime applies the canonicalized substring across entity/topic/fact, or conflict reason, after the exact filters. This is an implemented search mode, not a speculative enhancement.

### Independent adjudication

This omission has a concrete implemented public parameter, not a speculative search feature. query is parsed and combined with exact entity/topic/store filtering. Narrow 'case-insensitive' to ASCII case folding, since the implementation uses to_ascii_lowercase, not Unicode folding.

**`meerkat-mobkit/src/rpc/memory_methods.rs:626-640`**

```text
let query = match object.get("query") {
```

The parser accepts an optional string and includes it in MemoryQueryRequest.

**`meerkat-mobkit/src/runtime/memory.rs:196-198`**

```text
let token = raw.trim().to_ascii_lowercase();
```

The query is trimmed and ASCII-folded; an empty normalized query supplies no substring restriction.

**`meerkat-mobkit/src/runtime/memory.rs:377-415`**

```text
.filter(|assertion| assertion_matches_query(assertion))
```

Substring matching across entity/topic/fact, and entity/topic/reason for conflicts, is applied in conjunction with exact filters.

**`meerkat-mobkit/tests/memory_store.rs:138-161`**

```text
(Some(1), json!("double-check Recipient Consent"), json!([]),)
```

The RPC substring test matches mixed-case stored text using query='recipient consent'.

**Required correction:** Add optional query:string to memory/query and describe a trimmed, ASCII-case-insensitive substring across entity/topic/fact or conflict reason, combined with the exact filters. It is loaded-ledger text filtering, not semantic/vector search.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Added memory/query's optional query parameter and described trimmed ASCII-case-insensitive substring filtering over entity/topic/fact or conflict reason, conjunctive with exact canonical filters.

**Validation:** Checked parse_memory_query_params and runtime/memory.rs canonicalization/matching; explicitly avoided semantic/vector-search and Unicode-folding claims.

**Final review: pass.** query is documented with the actual trimmed ASCII substring semantics, assertion/conflict fields and conjunction with exact filters. The prose explicitly avoids semantic/vector-search and backend-health claims.

**`docs/api/rpc.mdx:766-780`**

```text
Trimmed, ASCII-case-insensitive substring
```

The query row names entity/topic/fact and conflict reason, including the normalized-empty behavior.

**`meerkat-mobkit/src/runtime/memory.rs:360-415`**

```text
.filter(|assertion| assertion_matches_query(assertion))
```

Read the closures and preceding exact filters. canonical_memory_token at 196-198 trims and ASCII-folds, and memory_methods.rs:626-639 actually parses the optional query string.

## C-010: The RPC memory introduction omits the required agent.memory.read grant

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:731-735`**

```text
Console callers can recall only identities they may view;
`remember` and `forget` also require mutating console access plus
`agent.memory.write` and `agent.memory.delete` respectively.
```

An operator grants agent.view as suggested but recall returns access_denied, especially for configurations that explicitly mention memory actions and therefore do not get the compatibility rewrite.

**`meerkat-mobkit/src/http_console.rs:2063-2069`**

```text
"mobkit/agent_memory/recall" | "mobkit/agent_memory/manifest" => Some(vec![
            (ACTION_AGENT_MEMORY_READ, identity.clone()),
            (ACTION_AGENT_VIEW, identity),
        ]),
```

Recall requires both read and view, not merely roster visibility. The access-control guide correctly states this, but the method reference lists only view while enumerating write/delete requirements.

### Independent adjudication

The quoted 'only identities they may view' is a true necessary condition, but in this explicit authorization summary it omits the independent read grant while enumerating write/delete requirements. This is materially misleading for non-migrated policies. The migration can make old view-only configurations work, so a correction must not claim every such deployment fails.

**`meerkat-mobkit/src/http_console.rs:2062-2069`**

```text
(ACTION_AGENT_MEMORY_READ, identity.clone()),
```

Recall and manifest require this action and ACTION_AGENT_VIEW on the same target.

**`meerkat-mobkit/src/http_console.rs:5460-5463`**

```text
console_rpc_access_violation(access_view, request.method.as_str(), &request.params)
```

The shared HTTP access gate executes this mapping before dispatch.

**`meerkat-mobkit/tests/access_control.rs:2942-2952`**

```text
json!("agent.memory.read"),
```

The existing HTTP test asserts -32030 specifically for the omitted memory-read grant under an explicit memory policy; the preceding test portion confirms legacy migration compatibility.

**Required correction:** State that, when ABAC is enforced, console recall/manifest require both agent.view and agent.memory.read on the identity. Keep the write/delete grants and mutation gate. Link or briefly explain compatibility normalization for memory-naive policies.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Documented both agent.view and agent.memory.read for ABAC recall/manifest, preserved mutation/write/delete requirements, and linked legacy policy normalization.

**Validation:** Checked console_rpc_access_requirements and the access-control migration documentation; local target page and heading resolve.

**Final review: pass.** The reference now requires both view and memory-read for recall/manifest under ABAC, preserves write/delete and mutation gates, and links the valid compatibility-normalization section rather than implying all older view-only policies break.

**`docs/api/rpc.mdx:786-794`**

```text
require both `agent.view` and `agent.memory.read` on the target identity;
```

The adjacent grant and migration wording is consistent and the local heading link resolves.

**`meerkat-mobkit/src/http_console.rs:2065-2069`**

```text
(ACTION_AGENT_MEMORY_READ, identity.clone()),
```

The same requirement list includes ACTION_AGENT_VIEW; the actual HTTP dispatcher runs the gate at 5460-5463.

## C-011: Console send replay is scoped and fingerprints handling_mode, not just key plus content

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:569-573`**

```text
Replaying a key with identical content returns the earlier interaction's
acceptance, whether or not that turn has finished; replaying it with different
content is refused with `-32009`
```

A reconnecting client that changes origin can send twice, or a client that changes handling_mode while preserving content can receive an unexpected conflict.

**`meerkat-mobkit/src/console_aggregator/mod.rs:1475-1487`**

```text
let dedupe_key = send_dedupe_key(
            &resolved.entry.runtime_key,
            &request.identity,
            &request.origin,
            &request.idempotency_key,
        );
```

A key is scoped by runtime, identity and origin, so changing origin or target can create another interaction rather than replaying the previous one.

**`meerkat-mobkit/src/console_aggregator/mod.rs:1486-1511`**

```text
send_request_fingerprint(&request.origin, &request.content, &handling_mode_value);
```

Within the same scope, handling_mode is part of the fingerprint. Identical content with queue changed to interrupt is a conflict, not an accepted replay.

### Independent adjudication

Both classic and identity-first sends scope the dedupe key by runtime, requested identity and origin, and both fingerprint handling_mode as well as content. The audit's illustrative 'interrupt' mode is not valid on the classic send path, so it is not a sound conflict example; queue versus steer is the correct supported example. Matching content alone does not establish a replay.

**`meerkat-mobkit/src/console_aggregator/mod.rs:5458-5469`**

```text
format!("send:{runtime_key}:{identity}:{origin}:{idempotency_key}")
```

The key is scoped, and the adjacent fingerprint hashes origin, mode and serialized content.

**`meerkat-mobkit/src/console_aggregator/mod.rs:1481-1511`**

```text
send_request_fingerprint(&request.origin, &request.content, &handling_mode_value);
```

The classic send path defaults omitted mode to queue and rejects mismatching fingerprints.

**`meerkat-mobkit/src/console_aggregator/mod.rs:1637-1653`**

```text
send_request_fingerprint(&request.origin, &request.content, &handling_mode_value);
```

Identity-first reservation uses the same scope and mode-sensitive fingerprint.

**`meerkat-mobkit/src/console_aggregator/mod.rs:5417-5424`**

```text
"steer" => Ok(meerkat_core::types::HandlingMode::Steer),
```

Only queue and steer are accepted by this parser; interrupt would fail mode validation before proving an idempotency conflict.

**Required correction:** Describe deduplication by (runtime, identity, origin, idempotency_key). A replay must retain origin, content and effective handling_mode (omitted means queue); changing queue to steer under the same scope conflicts. Changing target or origin can make a distinct send. Preserve the member-respawn persistence caveat, without promising replay after records or the runtime itself are lost.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Specified runtime/identity/origin/key deduplication scope and content/effective-mode fingerprinting, including omitted queue, queue-to-steer conflicts and distinct sends after target/origin changes. Preserved respawn continuity without promising survival of lost runtime/records.

**Validation:** Checked console_aggregator send_dedupe_key, send_request_fingerprint, both send paths and supported queue/steer parsing.

**Final review: pass.** The new send guidance correctly captures runtime/identity/origin/key scoping and content/effective-mode fingerprinting. The queue-to-steer example is supported, and the retained respawn caveat no longer implies persistence after records/runtime loss.

**`docs/api/rpc.mdx:586-597`**

```text
Deduplication is scoped by `(runtime, identity, origin, idempotency_key)`.
```

It explicitly distinguishes omitted queue, changed-mode conflicts, and distinct target/origin sends.

**`meerkat-mobkit/src/console_aggregator/mod.rs:5458-5469`**

```text
format!("send:{runtime_key}:{identity}:{origin}:{idempotency_key}")
```

The adjacent fingerprint hashes origin, mode and serialized content. Both send paths at 1476-1511 and 1637-1659 use these functions with the queue default.

## C-012: REST identities depends on the console aggregator, not an identity-first runtime

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rest.mdx:108-112`**

```text
Returns identity-first records and inspection metadata when an identity runtime is configured.

**Response:** JSON object containing identity records and affordances.
```

Worker-only or multi-runtime console embedders incorrectly think the route is unavailable; decoders expect affordances that the route does not return.

**`meerkat-mobkit/src/http_console.rs:943-971`**

```text
let Some(aggregator) = &state.console_aggregator else {
```

The route only requires a console aggregator, calls aggregator.list_identities, and returns {identities}; no identity-runtime prerequisite exists.

**`meerkat-mobkit/src/console_aggregator/types.rs:299-315`**

```text
pub struct ConsoleIdentityRecord {
    pub identity: String,
    pub display_name: String,
    pub runtime_key: String,
    pub runtime_member_id: String,
```

These are aggregator identity records (session_id, visibility, addressable, health, topology_peers, labels), not inspection records with an affordances member.

### Independent adjudication

The REST route is aggregator-backed, not conditional on IdentityRuntime. The aggregator explicitly includes ordinary live member identities when no identity runtime is attached. Its record schema has no affordances field. The proposed pointer to inspection also needs precision: console/inspect_identity returns identity plus peers, whereas experience carries affordances.

**`meerkat-mobkit/src/http_console.rs:943-969`**

```text
Json::<Value>(json!({ "identities": identities })),
```

The authenticated route returns aggregator.list_identities(), or 404 unavailable without an aggregator.

**`meerkat-mobkit/src/console_aggregator/mod.rs:2851-2897`**

```text
Box::pin(member_sources_for_entry(entry)).await
```

The branch without IdentityRuntime still collects visible member records.

**`meerkat-mobkit/src/console_aggregator/types.rs:303-325`**

```text
pub struct ConsoleIdentityInspection {
    pub identity: ConsoleIdentityRecord,
    #[serde(default)]
    pub peers: Vec<String>,
}
```

The preceding ConsoleIdentityRecord fields are identity/display/runtime/member/session/visibility/addressable/health/peers/labels; neither type exposes affordances.

**Required correction:** Describe the response as {identities: ConsoleIdentityRecord[]} from the console aggregator, including ordinary runtime/worker identities and visibility filtering. State 404 unavailable when no aggregator exists. Link console/inspect_identity for identity/peer inspection and /console/experience for affordances; do not promise affordances in the identities or inspect response.

### Changes and final verification

**Changed:** `docs/api/rest.mdx`, `docs/api/rpc.mdx`.

Replaced the identity-runtime prerequisite with aggregator-backed visibility-filtered ConsoleIdentityRecord[] and 404-unavailable behavior. Corrected the adjacent RPC inspection description to identity plus peers, reserving affordances for experience.

**Validation:** Checked console_identities_handler and ConsoleIdentityRecord/ConsoleIdentityInspection types. Local inspection/experience links resolve; no affordances field is promised in those records.

**Final review: pass.** The REST identities description now matches the aggregator-backed route, visibility filtering, no-aggregator 404 and concrete record fields. Its related RPC inspection description correctly separates identity/peers from experience affordances.

**`docs/api/rest.mdx:108-121`**

```text
**Response:** `{identities: ConsoleIdentityRecord[]}`.
```

The text allows ordinary workers, lists session_id/topology_peers omission rules and directs affordance consumers to experience. rpc.mdx:522-525 gives the separate inspection wrapper.

**`meerkat-mobkit/src/http_console.rs:943-969`**

```text
Json::<Value>(json!({ "identities": identities })),
```

The handler requires an aggregator, not IdentityRuntime, and filters returned rows. types.rs:304-324 confirms both record and inspection fields, with no affordances member.

## C-013: Timeline SSE does not use REST limit as a total replay bound

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/sse.mdx:61-61`**

```text
Streams a bounded replay snapshot followed by live console timeline frames. The route accepts the same `identity`, `conversation_id`, `after`, `before`, `mode`, and `limit` parameters as [`GET /console/timeline`](/api/rest).
```

Clients incorrectly budget one page of replay and assume the stream's initial window has REST default semantics.

**`meerkat-mobkit/src/http_console.rs:1626-1635`**

```text
if query.after.is_none() && query.mode == ConsoleTimelineMode::Since {
        query.mode = ConsoleTimelineMode::Recent;
    }
```

An unanchored stream defaults to a recent seed rather than REST's forward since query.

**`meerkat-mobkit/src/http_console.rs:1653-1676`**

```text
frames.extend(page.frames);
```

Anchored since replay loops over every page until exhaustion; limit controls each page, not the total snapshot.

**`meerkat-mobkit/src/http_console.rs:16606-16617`**

```text
limit: 1,
```

timeline_snapshot_drains_since_backlog_beyond_old_page_budget requests limit=1 and asserts frames.len()=149, explicitly pinning the unbounded-by-limit backlog drain.

### Independent adjudication

Although retained history is finite, 'bounded replay' beside REST's visible-frame limit falsely suggests limit bounds the entire snapshot. The actual stream drains every since page until exhausted. Without after it changes since to recent and returns one seed page. Header precedence is explicit rather than inferred from the misleading parameter name fallback_after.

**`meerkat-mobkit/src/http_console.rs:1626-1639`**

```text
if query.after.is_none() && query.mode == ConsoleTimelineMode::Since {
        query.mode = ConsoleTimelineMode::Recent;
    }
```

Unanchored default streams start with recent history, unlike the REST query's since default.

**`meerkat-mobkit/src/http_console.rs:1655-1675`**

```text
frames.extend(page.frames);
```

The since loop advances after and accumulates all retained matching pages; limit is per page.

**`meerkat-mobkit/src/http_console.rs:1600-1605`**

```text
let after = fallback_after.or(query.after).map(ConsoleCursor::from);
```

The nonempty Last-Event-ID supplied by the handler takes precedence over URL after.

**`meerkat-mobkit/src/http_console.rs:16606-16617`**

```text
assert_eq!(frames.len(), 149);
```

The existing backlog regression explicitly requests limit=1 and observes 149 replayed frames.

**Required correction:** Update both REST and SSE stream descriptions: recent produces a bounded seed; anchored since drains all retained matching backlog in pages, not a total limit-sized replay. Without after, since becomes recent. Document nonempty Last-Event-ID precedence over URL after and preserve cursor-error recovery.

### Changes and final verification

**Changed:** `docs/api/rest.mdx`, `docs/api/sse.mdx`.

Distinguished bounded recent seeds from anchored since replay draining retained matching backlog in pages; described unanchored since-to-recent conversion and nonempty Last-Event-ID precedence.

**Validation:** Checked timeline query parsing and snapshot loop, plus existing backlog regression source asserting 149 frames at limit=1. REST and SSE descriptions agree.

**Final review: pass.** REST and SSE now consistently distinguish a bounded recent seed from multi-page anchored-since replay. They preserve the unanchored-since conversion, nonempty Last-Event-ID precedence and stale-cursor recovery rather than conflating stream and page limits.

**`docs/api/sse.mdx:85-92`**

```text
pages, so `limit` controls each page, not total replay size.
```

rest.mdx:149-158 gives the same behavior and links to the stream-specific contract.

**`meerkat-mobkit/src/http_console.rs:1600-1678`**

```text
query.mode = ConsoleTimelineMode::Recent;
```

The parser uses fallback_after.or(query.after). The snapshot helper reads one recent page but loops, accumulates and advances next_cursor until since backlog is exhausted.

## C-014: The timeline SSE reference omits the envelope and event names needed to decode the stream

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/sse.mdx:55-75`**

```text
## Console timeline stream
```

A consumer following the agent-event example or assuming bare frames cannot access the frame payload or know when replay is complete.

**`meerkat-mobkit/src/http_console.rs:3191-3219`**

```text
ConsoleTimelineEvent::SnapshotStarted { .. } => ("snapshot_started", None),
```

The route sends snapshot_started, console_frame, frame_updated, snapshot_complete and potentially replay_unavailable event kinds. It serializes ConsoleTimelineEvent, not a bare ConsoleFrame or a raw AgentEvent.

**`meerkat-mobkit/src/console_aggregator/types.rs:370-390`**

```text
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConsoleTimelineEvent {
```

Frame messages use {type:"console_frame",frame:{...}} or {type:"frame_updated",frame:{...}}; markers have their own cursor/after fields. The existing section explains only route/query/errors, so an implementer has no published decoder shape for this primary console stream.

### Independent adjudication

The page's agent example cannot decode the separate console stream, and the section supplies neither its event names nor the frame wrapper. This is a concrete missing wire contract, not a request for generic detail. The correction must account for the serializer's special case: SSE event frame_updated can carry a type=console_frame payload whose frame.kind is frame_updated, not only type=frame_updated.

**`meerkat-mobkit/src/console_aggregator/types.rs:370-390`**

```text
#[serde(tag = "type", rename_all = "snake_case")]
```

SnapshotStarted, ConsoleFrame, FrameUpdated, SnapshotComplete and ReplayUnavailable are tagged envelope variants, not bare frames.

**`meerkat-mobkit/src/http_console.rs:3191-3217`**

```text
let data = match serde_json::to_string(event) {
```

The HTTP stream directly serializes the enum. Frame events carry cursor IDs, SnapshotComplete has an ID when its cursor exists, and the other markers have no ID.

**`meerkat-mobkit/src/http_console.rs:3194-3204`**

```text
if frame.kind == "frame_updated" {
```

SSE event name can be frame_updated even when data.type remains console_frame; clients must not assume those discriminators always coincide.

**`meerkat-mobkit/src/http_console.rs:1520-1535`**

```text
ConsoleTimelineEvent::SnapshotComplete { cursor: latest_cursor.clone() }
```

The actual route emits a snapshot-start marker, wrapped frames and a snapshot-complete marker before live continuation.

**Required correction:** Add a console-specific wire example and event table for snapshot_started, console_frame, frame_updated, snapshot_complete and replay_unavailable. Explain the type-tagged frame wrapper, optional marker cursors, cursor-bearing SSE IDs and Last-Event-ID. Explicitly allow frame_updated event names with data.type=console_frame and frame.kind=frame_updated as well as the FrameUpdated variant. Keep initial replay refusal as HTTP 409; reference the same contract from the identity stream.

### Changes and final verification

**Changed:** `docs/api/sse.mdx`.

Added a labeled abbreviated wrapped-frame example and all five timeline event names/shapes, optional marker cursors, SSE ID rules, replay refusal behavior and the frame_updated event-name/data.type exception. Identity stream references the same contract.

**Validation:** Compared ConsoleTimelineEvent and sse_event_from_timeline_event; concrete SSE data examples parse as JSON. Initial cursor refusal remains HTTP 409, not a guaranteed stream event.

**Final review: pass.** The new console SSE contract has the correct tagged wrapper, five variants, omission/null distinction and SSE ID rules. It handles the critical frame_updated name/type mismatch and labels the incomplete frame example as abbreviated. Initial refusal remains HTTP 409.

**`docs/api/sse.mdx:94-144`**

```text
The SSE event name
```

The event table and following sentence explicitly allow event frame_updated with data.type console_frame. The identity stream references this same contract.

**`meerkat-mobkit/src/console_aggregator/types.rs:370-390`**

```text
#[serde(tag = "type", rename_all = "snake_case")]
```

All five variants were compared, including omitted marker Option fields versus nullable latest_cursor.

**`meerkat-mobkit/src/http_console.rs:3191-3218`**

```text
if frame.kind == "frame_updated" {
```

The serializer chooses the SSE name independently, serializes the original enum, and only sets cursor-bearing IDs on the documented variants.

## C-015: The mob-merged SSE route is listed without its distinct wire shape or connection-local cursor semantics

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/sse.mdx:7-16`**

```text
- mob-merged streaming through `GET /mob/events`
```

Clients cannot correctly decode or filter the route, and can mistake its id for a resumable merged-event or structural cursor.

**`meerkat-mobkit/src/http_sse.rs:529-530`**

```text
let mut seq = 0_u64;
```

The sequence restarts per connection; this is not a structural ledger cursor.

**`meerkat-mobkit/src/http_sse.rs:598-609`**

```text
"member_id": &source,
                "source": &source,
                "payload": payload,
```

Mob-stream data wraps an agent payload with member_id and source, uses the agent event name and id mob:<seq>, and is not EventEnvelope<UnifiedEvent>. The entire assigned SSE page has no mob-stream section explaining this difference.

### Independent adjudication

GET /mob/events has a distinct per-source envelope and connection-local ID, neither explained in the page. I followed the registered handler through event projection and subscription; it does not consume Last-Event-ID as a replay cursor. This live agent-event merger is not the structural ledger or the bounded merged-event RPC snapshot.

**`meerkat-mobkit/src/http_sse.rs:386-393`**

```text
.route("/mob/events", get(mob_events_sse_handler))
```

This is the advertised live route and its independent state.

**`meerkat-mobkit/src/http_sse.rs:529-541`**

```text
let mut seq = 0_u64;
```

Each new connection initializes its own counter; source is rendered through runtime_event_alias and includes the runtime generation.

**`meerkat-mobkit/src/http_sse.rs:598-609`**

```text
"member_id": &source,
                "source": &source,
                "payload": payload,
```

Both member_id and source carry the attributed source, payload carries the projected agent event, and the ID is mob:<seq>.

**Required correction:** Add a /mob/events section with {member_id,source,payload}, an actual generation-suffixed source example, the agent event name, and mob:<connection-local sequence> IDs. State live-only/no Last-Event-ID replay and contrast it with structural ledger SSE and events/subscribe snapshots.

### Changes and final verification

**Changed:** `docs/api/sse.mdx`.

Added live /mob/events wire shape, generation-suffixed source example, connection-local mob:0 sequence and no Last-Event-ID replay; distinguished structural and merged RPC replay surfaces.

**Validation:** Checked http_sse.rs merger serialization/counter lifetime and member_comms_id runtime-event alias examples; the added data line parses as JSON.

**Final review: pass.** The added mob stream section correctly documents the distinct member_id/source/payload envelope, generation-suffixed aliases, event naming and connection-local sequence. It does not promise replay from a non-durable ID or confuse this route with the structural ledger.

**`docs/api/sse.mdx:55-79`**

```text
The `mob:<sequence>` ID is connection-local
```

The example starts at mob:0 and both source fields contain worker-1:0; the replay alternatives are explicitly separate surfaces.

**`meerkat-mobkit/src/http_sse.rs:529-547`**

```text
let mut seq = 0_u64;
```

The stream computes runtime_event_alias per source; lines 598-609 serialize the documented wrapper and increment the local mob sequence. No Last-Event-ID replay path exists in this handler.

## C-016: HTTP SSE keep-alive cadence is fixed, not configurable

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/sse.mdx:241-243`**

```text
The server sends keep-alive frames at a configurable interval (default: 15 seconds) to prevent connection timeouts:
```

Deployers look for a nonexistent heartbeat setting or assume configuration affects the stock route.

**`meerkat-mobkit/src/http_sse.rs:45-46`**

```text
pub(crate) const DEFAULT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);
```

All provided HTTP stream constructors pass this constant to KeepAlive::interval; their router/query/state APIs expose no interval setting. The timeline handler does the same at http_console.rs:1575-1577.

### Independent adjudication

The stock HTTP stream constructors all pass the same crate-private 15-second constant to Axum KeepAlive and expose no heartbeat configuration. The separate RPC snapshot's keep_alive metadata does not make this HTTP cadence configurable. State idle keep-alive behavior, not a guaranteed extra frame every 15 seconds during active traffic.

**`meerkat-mobkit/src/http_sse.rs:45-46`**

```text
pub(crate) const DEFAULT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);
```

This is a fixed internal constant, not a public configuration field.

**`meerkat-mobkit/src/http_sse.rs:613-617`**

```text
.interval(DEFAULT_KEEP_ALIVE_INTERVAL)
```

The mob stream applies the fixed interval; the agent, WorkGraph and structural constructors do the same.

**`meerkat-mobkit/src/http_console.rs:1573-1578`**

```text
.interval(DEFAULT_KEEP_ALIVE_INTERVAL)
```

The console timeline stream shares the same stock constant.

**Required correction:** Replace 'configurable interval (default: 15 seconds)' with a fixed 15-second idle keep-alive comment on the stock HTTP streams. Do not invent a configuration option; custom hosts can implement a different transport separately.

### Changes and final verification

**Changed:** `docs/api/sse.mdx`.

Replaced nonexistent heartbeat configurability with the stock fixed 15-second idle keep-alive comment behavior.

**Validation:** Checked DEFAULT_KEEP_ALIVE_INTERVAL and HTTP/timeline KeepAlive constructors; did not promise extra comments during active event traffic.

**Final review: pass.** Keep-alive guidance now states fixed 15-second idle comments, not a nonexistent exposed configuration or an unconditional extra frame every 15 seconds.

**`docs/api/sse.mdx:304-313`**

```text
time to prevent connection timeouts. This interval is fixed, not an exposed
```

The section identifies the interval as idle time and shows a comment frame.

**`meerkat-mobkit/src/http_sse.rs:45-46`**

```text
pub(crate) const DEFAULT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(15);
```

All stock stream constructors reference this constant, including console timeline at http_console.rs:1573-1578.

## C-017: The event guide directs wire clients to nonexistent ConsoleFrame.data fields

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/events.mdx:86-86`**

```text
so `frame.data.error` and `frame.data.reason` name the failure there too.
```

A wire consumer reads undefined/null error information and loses the typed failure details the section intends to expose.

**`meerkat-mobkit/src/console_aggregator/types.rs:112-149`**

```text
pub payload: Value,
```

ConsoleFrame's serialized payload field has no serde rename to data. Timeline REST/RPC and SSE serialize this type directly.

**`meerkat-mobkit/src/http_console.rs:3211-3215`**

```text
let data = match serde_json::to_string(event) {
```

SSE data contains the type-tagged timeline event, whose frame retains payload. It does not rewrite the nested field to data.

### Independent adjudication

The guide is discussing the console frame on wire surfaces. ConsoleFrame has payload, not data, and the stream serializes the frame inside its enum unchanged. UI-adapted data fields and the outer SSE data line are distinct representations and do not justify frame.data in this paragraph.

**`meerkat-mobkit/src/console_aggregator/types.rs:112-129`**

```text
pub payload: Value,
```

No serde rename turns payload into data.

**`meerkat-mobkit/src/http_console.rs:3211-3215`**

```text
let data = match serde_json::to_string(event) {
```

Timeline SSE preserves the nested ConsoleFrame field names.

**`meerkat-mobkit/src/http_console.rs:5249-5255`**

```text
Some(serde_json::to_value(page).unwrap_or(Value::Null)),
```

The RPC timeline page likewise serializes typed frames without a data-field adapter.

**Required correction:** Use frame.payload.error and frame.payload.reason for wire ConsoleFrame consumers. If retaining UI examples, explicitly distinguish their adapted data representation from REST/RPC/SSE frame payload.

### Changes and final verification

**Changed:** `docs/concepts/events.mdx`.

Changed wire-frame failure accessors to frame.payload.error/reason and explicitly named REST/RPC/SSE timeline frames.

**Validation:** Checked ConsoleFrame.payload and direct timeline enum/page serialization; preserved surrounding failure semantics.

**Final review: pass.** The changed accessor is precisely the wire payload field and is explicitly scoped to REST/RPC/SSE frames; the surrounding failure projection distinctions are preserved.

**`docs/concepts/events.mdx:86-86`**

```text
wire consumers read `frame.payload.error` and `frame.payload.reason`
```

No UI-adapter data representation is incorrectly promoted to the wire.

**`meerkat-mobkit/src/console_aggregator/types.rs:112-129`**

```text
pub payload: Value,
```

ConsoleFrame has no data rename; http_console.rs:3211 directly serializes the enclosing timeline event.

## C-018: The copyable access.toml example references an undefined contractors group

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/access-control.mdx:86-93`**

```text
id = "contractors-payments-readonly"
groups = ["contractors"]
```

Following the introductory configuration example verbatim prevents access configuration from loading.

**`meerkat-mobkit/src/access/model.rs:365-372`**

```text
if !config.groups.contains_key(group) {
                return Err(AccessConfigError::UnknownGroup {
```

Validation rejects references to an undefined group. The complete example defines only groups.ops.

**`meerkat-mobkit/src/access/controller.rs:66-74`**

```text
validate_access_config(&config)?;
```

Loading the file constructs a controller through this validation, so this is a startup/configuration failure rather than simply an unused rule.

### Independent adjudication

I parsed the full introductory TOML block independently: it defines only ops, while the contractor rule refers to contractors. The validator rejects unknown group references and the file-loading constructor invokes it. This is a copyable configuration failure, not merely a rule that matches nobody.

**`docs/concepts/access-control.mdx:65-101`**

```text
groups = ["contractors"]
```

A read-only Python tomllib check of the entire block returned unknown groups ['contractors'].

**`meerkat-mobkit/src/access/model.rs:365-371`**

```text
return Err(AccessConfigError::UnknownGroup {
```

Unknown groups invalidate the configuration.

**`meerkat-mobkit/src/access/controller.rs:69-74`**

```text
validate_access_config(&config)?;
```

Controller creation, including load_or_default, cannot accept this example.

**`meerkat-mobkit/src/access/model.rs:437-441`**

```text
Err(AccessConfigError::UnknownGroup { .. })
```

An existing unit test pins the rejection.

**Required correction:** Define [groups.contractors] with illustrative members alongside [groups.ops], before the rules, preserving the contractor/payments read-only demonstration.

### Changes and final verification

**Changed:** `docs/concepts/access-control.mdx`.

Added a contractors group with illustrative membership before the existing contractor/payments rule.

**Validation:** Python tomllib parses the complete first TOML block; every referenced group now exists and enabled/admin prerequisites remain satisfied.

**Final review: pass.** The complete introductory TOML now parses and all rule group references exist. The new contractors group preserves the intended payments read-only demonstration and enabled/admin prerequisites.

**`docs/concepts/access-control.mdx:72-111`**

```text
[groups.contractors]
```

An independent tomllib parse and set comparison found no undefined group, including contractors.

**`meerkat-mobkit/src/access/model.rs:365-371`**

```text
if !config.groups.contains_key(group) {
```

This is the exact validation failure the fix eliminates; controller construction at controller.rs:69-74 normalizes then validates the config.

## C-019: The advertised fixed access-action vocabulary omits both WorkGraph actions

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/access-control.mdx:15-35`**

```text
- **Action** — a verb from a fixed vocabulary:
```

Operators cannot discover the grants required to make the WorkGraph panel, RPC family or wake stream usable from the central authorization reference.

**`meerkat-mobkit/src/access/model.rs:49-53`**

```text
pub const ACTION_WORKGRAPH_VIEW: &str = "workgraph.view";
/// Mutate WorkGraph state: goals, item lifecycle, attention operations.
pub const ACTION_WORKGRAPH_MANAGE: &str = "workgraph.manage";
```

Both constants are included in ACCESS_ACTIONS at 105-106, but neither appears in the guide's supposedly complete table.

**`meerkat-mobkit/src/http_console.rs:2160-2167`**

```text
one(ACTION_WORKGRAPH_MANAGE, None)
```

These are required resource-less grants on current public RPC methods; WorkGraph SSE additionally requires workgraph.view.

### Independent adjudication

The central table introduces a fixed action vocabulary but leaves out two active, validated actions. They gate real WorkGraph requests, unlike reserved/future entries that the table already includes. Existing HTTP tests prove view-only users may read but cannot create.

**`meerkat-mobkit/src/access/model.rs:50-53`**

```text
pub const ACTION_WORKGRAPH_MANAGE: &str = "workgraph.manage";
```

Both workgraph.view and workgraph.manage are defined and appear in ACCESS_ACTIONS at 105-106.

**`meerkat-mobkit/src/http_console.rs:2159-2165`**

```text
one(ACTION_WORKGRAPH_MANAGE, None)
```

Mutations and reads map to manage/view respectively with no agent resource.

**`meerkat-mobkit/tests/workgraph_rpc.rs:1855-1866`**

```text
assert_eq!(denied["error"]["data"]["action"], json!("workgraph.manage"));
```

The existing HTTP test successfully reads a snapshot under view and receives access_denied for create.

**Required correction:** Add workgraph.view and workgraph.manage to the action table. Describe their resource-less whole-WorkGraph scope; include reads/wake observation versus mutations and retain the independent console read-only mutation gate.

### Changes and final verification

**Changed:** `docs/concepts/access-control.mdx`.

Added workgraph.view/manage to the action table and described resource-less whole-WorkGraph authorization plus the independent console read-only mutation gate.

**Validation:** Checked access/model.rs action constants, HTTP WorkGraph action mapping and workgraph_methods.rs read/mutate catalogs.

**Final review: pass.** The central action table now includes both implemented WorkGraph actions, with whole-graph/resource-less semantics and the independent mutation-policy condition. The wake-stream assertion is backed by the actual SSE gate.

**`docs/concepts/access-control.mdx:35-36`**

```text
| `workgraph.view` | Read WorkGraph snapshots, items, attention, events, and facts; observe the wake stream |
```

Lines 52-54 qualify scope and read-only behavior.

**`meerkat-mobkit/src/http_console.rs:2157-2166`**

```text
one(ACTION_WORKGRAPH_MANAGE, None)
```

Read and mutation catalogs map to view/manage with no resource. http_sse.rs:941-945 requires ACTION_WORKGRAPH_VIEW for the WorkGraph fact/wake stream.

## C-020: An open console is not always anonymous when a valid optional token is supplied

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/access-control.mdx:13-13`**

```text
On an open console (`require_app_auth = false`) the principal is anonymous and only matches rules without subject constraints.
```

Embedders incorrectly expect named-user grants to be ignored in optional-auth mode and cannot reason about why two callers see different data.

**`meerkat-mobkit/src/http_console.rs:3670-3685`**

```text
let principal = state.access.as_ref().and_then(|_| {
            let token = console_request_token(headers, uri)?;
            resolve_authorized_console_auth_from_token(&state.decisions, &token)
                .map(|auth| auth.email)
        });
```

With access control attached, the open HTTP console identifies valid voluntary tokens and applies subject/group grants.

**`meerkat-mobkit/src/http_sse.rs:1096-1104`**

```text
let subject = token.as_deref().and_then(|token| {
```

The open SSE path has the same optional identification behavior.

### Independent adjudication

Auth-optional does not mean always anonymous. The HTTP ABAC path and SSE path deliberately resolve valid volunteered credentials. I also followed the token resolver: it validates trusted OIDC/JWKS and route policy rather than merely decoding a subject. Callers without valid credentials still become anonymous in this optional-auth mode.

**`meerkat-mobkit/src/http_console.rs:3670-3684`**

```text
let principal = state.access.as_ref().and_then(|_| {
```

With an access controller, an open HTTP console resolves an optional request token and uses its validated email as the principal.

**`meerkat-mobkit/src/http_sse.rs:1095-1102`**

```text
return Ok(access.map(|controller| controller.view_for_subject(subject.as_deref())));
```

Open SSE identifies valid volunteered tokens and otherwise evaluates the anonymous view.

**`meerkat-mobkit/src/runtime/console_ingress.rs:1175-1182`**

```text
let auth = resolve_console_auth_from_token(decisions, token).ok()?;
```

Optional identification still depends on the configured real token validation and route-access policy.

**`meerkat-mobkit/src/access/controller.rs:245-252`**

```text
None => AccessPrincipal::anonymous(),
```

Anonymous is specifically the absent-subject case, not the universal open-console principal.

**Required correction:** Qualify the principal definition and relationship-to-authentication paragraph: when authentication is optional, a caller lacking a valid volunteered token is anonymous; a valid token can identify a named principal for ABAC when trusted-token validation and access control are configured. Keep the recommendation to require auth for per-user deployments.

### Changes and final verification

**Changed:** `docs/concepts/access-control.mdx`.

Qualified both optional-auth statements: absent/invalid volunteered credentials yield anonymous access, while valid trusted tokens can identify named ABAC principals. Kept the recommendation to require auth for per-user deployments.

**Validation:** Checked console_request_auth_context, optional-auth SSE resolution and controller.view_for_subject.

**Final review: pass.** Both repeated optional-auth claims are corrected without implying unvalidated token decoding or universally named callers. They preserve anonymous fallback and recommend required authentication for per-user deployments.

**`docs/concepts/access-control.mdx:13-13`**

```text
callers without a valid voluntarily supplied token are anonymous
```

The relationship-to-authentication section at 196-198 repeats the trusted-token/access-control prerequisites consistently.

**`meerkat-mobkit/src/http_console.rs:3670-3684`**

```text
resolve_authorized_console_auth_from_token(&state.decisions, &token)
```

Optional HTTP identification uses validated route-authorized credentials when access exists; http_sse.rs:1095-1102 does the corresponding optional token resolution before building the view.

## C-021: Memory-action migration does not write the access file on load

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/access-control.mdx:170-170`**

```text
The rewrite is materialized into the persisted config, so it happens once.
```

Operators assume a boot permanently migrated the file; without an admin save, the same old file is re-normalized and can warn on each restart.

**`meerkat-mobkit/src/access/controller.rs:96-114`**

```text
let controller = Self::new(config)?;
```

load_or_default normalizes the in-memory configuration and records persist_path, but does not commit or write it.

**`meerkat-mobkit/src/access/model.rs:298-301`**

```text
persisted TOML on the next admin save
```

The normalization owner's own contract explicitly states when the migrated rule list reaches disk; persistence happens in controller.commit during mutation.

### Independent adjudication

load_or_default constructs and normalizes the controller, then records a persistence path; it performs no write. Persistence occurs during a subsequent committed admin mutation. The source comment expressly describes that timing. The existing self-limiting property applies to the normalized config, not to an unchanged file on every restart.

**`meerkat-mobkit/src/access/controller.rs:96-112`**

```text
let controller = Self::new(config)?;
```

The loader only creates the normalized controller and assigns persist_path before returning.

**`meerkat-mobkit/src/access/controller.rs:223-232`**

```text
persist_config(&path, &config)?;
```

Disk persistence is in commit, reached after mutations, not on load.

**`meerkat-mobkit/src/access/model.rs:297-300`**

```text
/// persisted TOML on the next admin save), which also makes the rewrite
```

The normalization contract explicitly agrees with the implementation.

**Required correction:** Say normalization takes place in memory on load and reaches the TOML file on the next successful persisted admin mutation/save. Until then an unchanged old file is normalized again on restart. Avoid implying load itself writes the file.

### Changes and final verification

**Changed:** `docs/concepts/access-control.mdx`.

Clarified that legacy memory-rule normalization is in-memory on load and persists only on the next successful persisted admin mutation/save; unchanged files normalize again after restart.

**Validation:** Compared AccessController::load_or_default with commit/persist_config and the normalization owner's contract.

**Final review: pass.** The migration paragraph now distinguishes load-time in-memory normalization from later successful persisted mutation/save, including repeat normalization of an unchanged file after restart.

**`docs/concepts/access-control.mdx:186-186`**

```text
Normalization is in memory on load
```

The rest of the sentence correctly conditions disk changes on the next persisted admin mutation/save.

**`meerkat-mobkit/src/access/controller.rs:96-112`**

```text
let controller = Self::new(config)?;
```

Load only attaches the persistence path after construction; commit at 223-239 performs persist_config before replacing state and advancing revision.

## C-022: The cross-mob grant reference omits the two explicit host-plane verbs

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/concepts/access-control.mdx:189-196`**

```text
Peer gateways drive four verbs across it:
```

A host/placement integrator cannot configure the required explicit host grants from this reference and may incorrectly expect verbs=['*'] to grant them.

**`meerkat-mobkit/src/runtime/cross_mob_control.rs:660-669`**

```text
pub enum ControlVerb {
    Wire,
    Unwire,
    Inject,
    LookupMember,
    /// Runtime-host identity, capability and placement-label projection.
    HostDescribe,
    /// Runtime-host health projection.
    HostHealth,
}
```

The actual channel accepts six verbs; host_describe and host_health expose runtime-host projections, not members.

**`meerkat-mobkit/src/runtime/cross_mob_control.rs:1281-1291`**

```text
verbs.extend(ControlVerb::member_plane());
```

The wildcard deliberately remains restricted to the four member-plane verbs, so host access must be explicitly named. The guide's 'all four' statement is only correct when scoped to the member plane.

### Independent adjudication

The four member-plane verbs are real but no longer exhaust the control channel. HostDescribe/HostHealth are parsed, authorized and dispatched, while wildcard expansion deliberately preserves the old four-verb authority. Adding these entries must not widen the documented meaning of '*', nor suggest host verbs address a member.

**`meerkat-mobkit/src/runtime/cross_mob_control.rs:658-668`**

```text
HostDescribe,
    /// Runtime-host health projection.
    HostHealth,
```

The serialized enum has six verbs, including the two host-plane operations.

**`meerkat-mobkit/src/runtime/cross_mob_control.rs:1282-1288`**

```text
verbs.extend(ControlVerb::member_plane());
```

The actual TOML parser expands '*' only to the original member plane.

**`meerkat-mobkit/src/runtime/cross_mob_control.rs:1961-1977`**

```text
host_describe_response(provider.as_deref())
```

There are real host dispatch branches; availability still depends on an installed host-facts provider.

**`meerkat-mobkit/src/runtime/cross_mob_control.rs:3419-3438`**

```text
fn star_verbs_never_widen_to_the_host_plane() {
```

A dedicated test pins the wildcard's member-only scope; adjacent tests admit explicit HostDescribe independently of member selectors.

**Required correction:** Describe four member-plane verbs plus host_describe and host_health. Host verbs must be explicitly granted, address the host rather than a member, and need host-facts support to return data. State '*' remains member-plane-only. Preserve signature, audience and member-scope rules for the member plane.

### Changes and final verification

**Changed:** `docs/concepts/access-control.mdx`.

Separated four member-plane verbs from explicit host_describe/host_health grants, documented host-facts support, and preserved member-only wildcard and member-selector semantics.

**Validation:** Checked ControlVerb, member_plane wildcard expansion, host dispatch/provider requirement and existing host-grant regression source.

**Final review: pass.** The reference now separates all four member-plane verbs from the two host-plane verbs and explicitly preserves the non-widening wildcard rule. Provider availability and lack of member-selector scoping for host operations are retained.

**`docs/concepts/access-control.mdx:204-249`**

```text
`verbs = ["*"]` grants only the four member-plane verbs, never
```

The preceding table adds host_describe/host_health with explicit-grant and installed-provider qualifications.

**`meerkat-mobkit/src/runtime/cross_mob_control.rs:1282-1288`**

```text
verbs.extend(ControlVerb::member_plane());
```

The actual grant parser does not expand to all six. The enum at 658-668 and dispatch/provider helpers at 1846-1867,1965-1977 implement the two separately granted host operations.

## C-023: The current console contract marks required origin and idempotency_key fields optional

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/rct/console-rest-sse-contract-v0.5.0.json:161-171`**

```text
"optional_fields": [
            "origin",
            "idempotency_key",
            "handling_mode"
          ]
```

A generated client or validator following the canonical current contract constructs invalid sends.

**`meerkat-mobkit/src/console_aggregator/types.rs:326-334`**

```text
pub struct ConsoleSendRequest {
    pub identity: String,
    pub content: Value,
    pub origin: String,
    pub idempotency_key: String,
```

origin and idempotency_key are non-defaulted String fields. Only handling_mode has serde default.

**`meerkat-mobkit/src/http_console.rs:6181-6188`**

```text
let send_request: ConsoleSendRequest =
                match serde_json::from_value(request.params.clone()) {
```

HTTP RPC directly deserializes this type, returning -32602 for missing fields. REST likewise extracts Json<ConsoleSendRequest>. The same erroneous optional list appears in the RPC method entry at 333-337.

### Independent adjudication

v0.5.0 is the currently selected canonical console contract, not a retired API snapshot merely because its filename is versioned. Its required_fields lists contradict the type directly deserialized by HTTP RPC and REST. The older v0.1-v0.4 files should not be changed to follow this current behavior.

**`meerkat-mobkit/tests/console_route_auth.rs:277-285`**

```text
"../../docs/rct/console-rest-sse-contract-v0.5.0.json"
```

The active version-pinning test selects v0.5.0, agreeing with its canonical-source note and MOBKIT_CONTRACT_VERSION.

**`meerkat-mobkit/src/console_aggregator/types.rs:327-334`**

```text
pub origin: String,
    pub idempotency_key: String,
```

These fields have no serde defaults; only handling_mode is optional/defaulted.

**`meerkat-mobkit/src/http_console.rs:6181-6187`**

```text
match serde_json::from_value(request.params.clone()) {
```

The runtime HTTP console/send arm deserializes ConsoleSendRequest directly; the REST route uses Json<ConsoleSendRequest> at 1036.

**`meerkat-mobkit/src/console_aggregator/mod.rs:5390-5398`**

```text
"idempotency_key must be non-empty".to_string(),
```

Both origin and key must additionally be non-empty after trimming, including on the identity-first reservation path.

**Required correction:** In v0.5.0 only, move origin and idempotency_key to required_fields for REST legacy_send and RPC mobkit/console/send. Keep handling_mode optional, and state that origin/key must be non-empty after trimming. Do not promise the same missing-field HTTP status on REST and JSON-RPC, since REST extraction is separate.

### Changes and final verification

**Changed:** `docs/rct/console-rest-sse-contract-v0.5.0.json`, `docs/api/rpc.mdx`.

Moved origin and idempotency_key into required fields for both canonical REST and RPC sends, retained optional handling_mode, and stated nonempty-after-trimming validation without equating REST extraction and RPC missing-field statuses.

**Validation:** Compared ConsoleSendRequest and validation; Python asserts exact required/optional sets on both v0.5 entries.

**Final review: pass.** Both current canonical send requests now require identity/content/origin/idempotency_key and leave only handling_mode optional. The prose states trimmed-nonempty validation without incorrectly equating REST extraction and JSON-RPC error statuses.

**`docs/rct/console-rest-sse-contract-v0.5.0.json:160-174`**

```text
"origin",
```

Independent assertions checked exact required/optional sets here and in methods.mobkit/console/send. rpc.mdx:586 also states the validation rule.

**`meerkat-mobkit/src/console_aggregator/types.rs:327-334`**

```text
pub idempotency_key: String,
```

origin and key have no serde defaults; only handling_mode is optional/defaulted. mod.rs:5384-5398 enforces nonempty trimmed strings.

## C-024: The canonical blob-upload contract requires an unused field and omits the required upload identifier

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/rct/console-rest-sse-contract-v0.5.0.json:429-447`**

```text
"required_fields": [
              "content_type"
            ]
```

An upload created from the contract fails for missing upload_id/part_name, and consumers look for the wrong MIME field in a successful response.

**`meerkat-mobkit/src/http_console.rs:3823-3873`**

```text
let upload = params.get("upload").unwrap_or(params);
```

The handler expects an upload object, or the same fields directly in params, containing upload_id or part_name matching exactly one file:<id> multipart part. media_type is optional and checked against the file MIME type. content_type is never read.

**`meerkat-mobkit/src/http_console.rs:3868-3873`**

```text
"blob_id": blob_ref.blob_id,
        "media_type": blob_ref.media_type,
        "size": size,
```

The returned fields are blob_id, media_type and size, not the contract's optional content_type/url inventory.

**`meerkat-mobkit/src/http_console.rs:16761-16777`**

```text
"upload_id": "upload-1",
                    "media_type": "image/png"
```

multipart_blob_upload_stores_one_file exercises the actual shape and asserts the media_type and size result fields.

### Independent adjudication

The current canonical blob schema has neither the required selector nor the actual response inventory. HTTP reads the MIME type from the multipart file's Content-Type header; it does not read params.content_type. The params may contain an upload wrapper or the same fields directly, and upload_id/part_name identify the single file part.

**`meerkat-mobkit/src/http_console.rs:3737-3747`**

```text
.get("upload_id")
        .or_else(|| object.get("part_name"))
```

One non-empty identifier is mandatory, with upload_id preferred when present.

**`meerkat-mobkit/src/http_console.rs:3823-3872`**

```text
let upload = params.get("upload").unwrap_or(params);
```

The extractor validates optional type/media_type, exactly one matching file, and returns blob_id/media_type/size.

**`meerkat-mobkit/src/http_console.rs:3262-3290`**

```text
let Some(upload_id) = name.strip_prefix("file:").filter(|id| !id.is_empty()) else {
```

The multipart transport expects a JSON-RPC payload part and file:<id> bytes, not a params.content_type contract.

**`meerkat-mobkit/src/http_console.rs:16762-16777`**

```text
assert_eq!(result["media_type"], json!("image/png"));
```

The existing upload test sends upload.upload_id and verifies media_type plus size.

**Required correction:** Fix the v0.5.0 blob/upload schema to accept params.upload (or direct params) with required upload_id or part_name, optional type constrained to image_upload and optional media_type matching the file MIME. Explain the JSON-RPC payload part and exactly one file:<id> part. Mark blob_id, media_type and size as returned fields; do not promise url/content_type. A small matching example can be shared by the terse REST/RPC upload descriptions. Retain png/jpeg/webp/gif and the 25 MiB per-file limit.

### Changes and final verification

**Changed:** `docs/rct/console-rest-sse-contract-v0.5.0.json`, `docs/api/rest.mdx`, `docs/api/rpc.mdx`.

Replaced the invented blob content_type schema with upload/direct params, upload_id-or-part_name selector, optional type/media_type, a JSON-RPC payload part and exactly one matching file part. Documented png/jpeg/webp/gif, 25 MiB, and returned blob_id/media_type/size with a shared concrete example.

**Validation:** Checked multipart handler, image_upload_part_name, externalize_single_image_upload and existing upload test source; JSON assertions verify canonical selector/result fields and all local example links.

**Final review: pass.** The canonical upload schema, REST example and RPC summary agree on the selector/wrapper, exactly one matching file part and blob_id/media_type/size result. MIME and 25 MiB bounds match the transport. The JSON contract correctly describes non-string optional-field permissiveness rather than inventing stricter runtime validation.

**`docs/api/rest.mdx:183-211`**

```text
The result contains `blob_id`, `media_type`, and `size` in bytes, not `url`
```

The supplied JSON parses and its upload-1 selector matches the instructed file:upload-1 part. Canonical JSON success keys were independently asserted.

**`meerkat-mobkit/src/http_console.rs:3823-3872`**

```text
let upload = params.get("upload").unwrap_or(params);
```

Read the entire extractor: selected upload_id/part_name, optional string type/media_type, one-file enforcement, MIME equality and the exact three result fields. Lines 3737-3747 establish selector precedence, and 246/3352 establish the size bound.

## C-025: The current HTTP delivery-history contract invents cursor pagination

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/rct/console-rest-sse-contract-v0.5.0.json:559-566`**

```text
"optional_fields": [
              "limit",
              "cursor"
            ]
```

A client tries to advance pages with cursor and repeatedly receives the same bounded history.

**`meerkat-mobkit/src/http_console.rs:7319-7348`**

```text
.delivery_history(DeliveryHistoryRequest {
                    recipient: None,
                    sink: None,
                    limit,
                });
```

The HTTP handler reads limit (default 50) only and constructs a non-cursor request. Supplying cursor is silently ignored.

**`meerkat-mobkit/src/runtime.rs:763-775`**

```text
pub struct DeliveryHistoryRequest {
```

The public request contains recipient, sink and limit, and the response contains deliveries; neither has a cursor field. Stdio adds exact recipient/sink filters but no cursor pagination.

### Independent adjudication

The HTTP arm extracts only limit, supplies no recipient/sink filters and has no cursor parameter or response cursor. The operational stdio parser has additional filters and a different default but still no cursor. This is not a renamed or opaque pagination token hidden elsewhere in dispatch.

**`meerkat-mobkit/src/http_console.rs:7331-7347`**

```text
.unwrap_or(50) as usize;
```

HTTP constructs DeliveryHistoryRequest with recipient=None, sink=None and limit; cursor is ignored.

**`meerkat-mobkit/src/runtime.rs:763-775`**

```text
pub struct DeliveryHistoryResponse {
    pub deliveries: Vec<DeliveryRecord>,
}
```

Neither request nor response carries a cursor.

**`meerkat-mobkit/src/rpc/routing_delivery_methods.rs:408-421`**

```text
limit: 20,
```

The stdio null-params default is 20; the following parser accepts recipient and sink, not cursor.

**Required correction:** Remove cursor from the current v0.5.0 HTTP delivery/history contract and describe a bounded history snapshot with HTTP default limit 50. If documenting stdio alongside it, distinguish recipient/sink filters and its default 20. Do not invent pagination or change older contract snapshots.

### Changes and final verification

**Changed:** `docs/rct/console-rest-sse-contract-v0.5.0.json`, `docs/api/rpc.mdx`.

Removed delivery-history cursor pagination, documented bounded HTTP snapshots with default limit 50 and distinct stdio recipient/sink filters with default 20.

**Validation:** Compared the HTTP DeliveryHistoryRequest construction, stdio parser and DeliveryHistoryResponse; JSON assertion confirms limit-only HTTP optional fields.

**Final review: pass.** Cursor pagination is removed from the current HTTP schema. The added reference precisely describes the deliveries wrapper and HTTP limit=50 versus stdio limit=20 with recipient/sink filtering.

**`docs/api/rpc.mdx:984-989`**

```text
Return `{deliveries: DeliveryRecord[]}` as a bounded history snapshot
```

Independent JSON assertions verified the canonical request is limit-only and the success requires deliveries.

**`meerkat-mobkit/src/http_console.rs:7331-7347`**

```text
.unwrap_or(50) as usize;
```

HTTP explicitly supplies recipient=None/sink=None and serializes DeliveryHistoryResponse. The complete stdio parser at routing_delivery_methods.rs:408-460 uses default 20 and the two optional filters, without any cursor.

## C-026: Access preview's subject is optional so administrators can preview anonymous access

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/rct/console-rest-sse-contract-v0.5.0.json:818-828`**

```text
"required_fields": [
              "subject",
              "action"
            ]
```

Clients generated from the contract cannot express the supported anonymous-policy preview.

**`meerkat-mobkit/src/http_console.rs:3145-3155`**

```text
let subject = request.params.get("subject").and_then(Value::as_str);
```

Only action is required; absent/null subject is passed as None to controller.view_for_subject and evaluates an anonymous principal.

### Independent adjudication

subject selects the principal being previewed, not the caller's authority to use the admin method. The caller still needs admin access, but omission/null is deliberately passed as None and previews anonymous access. Only action is required by this branch.

**`meerkat-mobkit/src/http_console.rs:3145-3155`**

```text
let subject = request.params.get("subject").and_then(Value::as_str);
```

No required-field error exists for subject; absent/null values become None.

**`meerkat-mobkit/src/access/controller.rs:245-252`**

```text
None => AccessPrincipal::anonymous(),
```

controller.view_for_subject(None) evaluates an anonymous principal.

**Required correction:** In the current v0.5.0 access/preview request, keep action required and move subject to optional_fields beside identity. Say omission/null previews anonymous access without relaxing the caller's access.admin/admin requirement.

### Changes and final verification

**Changed:** `docs/rct/console-rest-sse-contract-v0.5.0.json`.

Made preview subject optional alongside identity, kept action required, and documented omitted/null anonymous preview without changing administrator authorization.

**Validation:** Checked the preview branch and view_for_subject(None); JSON assertions verify required/optional lists.

**Final review: pass.** The canonical preview request now requires only action, permits optional/null subject for anonymous preview, and preserves the caller's separate administrative authorization.

**`docs/rct/console-rest-sse-contract-v0.5.0.json:984-995`**

```text
"note": "Omitted/null subject previews anonymous access. The caller still needs access.admin or admin standing."
```

The parsed required/optional field arrays were independently asserted.

**`meerkat-mobkit/src/http_console.rs:3145-3155`**

```text
let subject = request.params.get("subject").and_then(Value::as_str);
```

The branch passes None into view_for_subject for missing/null subject; controller.rs:252 creates the anonymous principal. The enclosing admin check runs at 3057-3065.

## C-027: The canonical console contract omits current authorization and admission failure variants

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/rct/console-rest-sse-contract-v0.5.0.json:343-355`**

```text
"codes": [
                -32001,
                -32002,
                -32004,
                -32009,
                -32000,
                -32602
              ]
```

Contract-driven clients misclassify ordinary read-only/ABAC/backpressure responses as unknown failures and lack the discriminators needed for correct error handling.

**`meerkat-mobkit/src/http_console.rs:1758-1767`**

```text
code: -32010,
            message: "console is read-only".to_string(),
            data: Some(json!({ "kind": "read_only" })),
```

Every classified HTTP console mutation can return this read-only error, but the current send/multipart/mutation contracts do not include it. This -32010 meaning also differs from the RPC page's stale-mob-cursor entry.

**`meerkat-mobkit/src/http_console.rs:1058-1074`**

```text
StatusCode::FORBIDDEN,
            "access_denied",
```

REST send returns 403 for ABAC; read-only also returns 403. Those statuses are absent from the legacy_send error inventory.

**`meerkat-mobkit/src/http_console.rs:1771-1798`**

```text
(StatusCode::TOO_MANY_REQUESTS, "admission_backlog_full")
```

Structured identity send failures add 429 backlog-full, 503 actor-probe/reload refusal, and 504 admission/reload timeout responses with data. The canonical REST send inventory ends at generic 500 and does not describe them.

**`meerkat-mobkit/src/http_console.rs:5455-5466`**

```text
console_rpc_access_violation(access_view, request.method.as_str(), &request.params)
```

Shared access checks can return -32030 before send, lifecycle, gating and observation dispatch; several of those method error inventories omit it.

### Independent adjudication

These are reachable current policy and identity-admission responses omitted from the selected canonical contract. I checked both common HTTP pre-dispatch gates and the REST send error mapper, plus existing typed-error assertions. Add shared policy semantics conditionally: read-only applies only to classified mutations, and ABAC can filter some read results instead of refusing them. Do not blindly add -32030 to every method or retrofit prior versions.

**`meerkat-mobkit/src/http_console.rs:1758-1766`**

```text
data: Some(json!({ "kind": "read_only" })),
```

The read-only policy error is -32010, distinct by context/data from the structural stale-cursor use of that number.

**`meerkat-mobkit/src/http_console.rs:5445-5463`**

```text
return console_read_only_rpc_error(response_id);
```

The HTTP dispatcher enforces read-only and then ABAC before method execution. Aggregator and multipart paths also apply these policy seams.

**`meerkat-mobkit/src/http_console.rs:1045-1073`**

```text
return console_json_error(StatusCode::FORBIDDEN, "read_only", "console is read-only");
```

REST send also emits 403 for read-only and for agent.send denial.

**`meerkat-mobkit/src/http_console.rs:1770-1803`**

```text
(StatusCode::TOO_MANY_REQUESTS, "admission_backlog_full")
```

Structured send errors map backlog to 429, probe/reload refusal to 503, and admission/reload timeout to 504, preserving data.

**`meerkat-mobkit/src/http_console.rs:11543-11563`**

```text
json!({"kind": "mob_member_reload_timed_out", "stage": "resume_lifecycle"}),
```

Existing tests pin the typed data/status mappings for backlog, reload refusal and reload timeout.

**`meerkat-mobkit/tests/workgraph_rpc.rs:1864-1866`**

```text
assert_eq!(denied["error"]["data"]["kind"], json!("access_denied"));
```

The shared -32030 access-denied contract is exercised through an actual HTTP method.

**Required correction:** In v0.5.0 add shared HTTP policy errors with applicability: -32010/data.kind=read_only for classified mutations; -32030/data.kind=access_denied for applicable ABAC gates. Reference/include them in affected inventories, including send and multipart. Add REST-send 403 plus structured 429 admission_backlog_full, 503 actor_probe_unhealthy/reload_refused, and 504 admission_timeout/reload_timed_out variants with message/data. Explain the method/data distinction for the reused -32010 number in rpc.mdx; do not claim every -32010 is a stale cursor. Preserve historical contracts.

### Changes and final verification

**Changed:** `docs/rct/console-rest-sse-contract-v0.5.0.json`, `docs/api/rpc.mdx`, `docs/api/rest.mdx`.

Added shared HTTP read_only/access_denied policy contracts and conditional references in affected endpoint/method inventories, including multipart and send. Added REST 403 and structured 429/503/504 admission variants (and the adjacent implemented structured host-human-input 409). Explained reused -32010 disambiguation and preserved fate/retry uncertainty.

**Validation:** Read-only Python checks every canonical method's read-only applicability against source mutation catalogs and verifies applicable ABAC inventories; no agent ABAC gate was invented for standalone blob upload or filtered reads. REST statuses and internal JSON pointer targets validate.

**Final review: pass.** Shared policy definitions now supplement affected inventories conditionally, including multipart without inventing agent.send authorization for standalone blob upload. The REST additions have the correct statuses/discriminators and preserve structured data/fate uncertainty. The reused -32010 code is explicitly disambiguated.

**`docs/rct/console-rest-sse-contract-v0.5.0.json:304-356`**

```text
"policy_error_inventory": "policy_error_refs supplement each endpoint or method's local errors array. Endpoint references are conditional on the dispatched method, not all requests."
```

All policy JSON pointers resolve. An independent comparison checked read-only applicability for all 50 canonical methods against direct and delegated mutation catalogs; gated ABAC inventories contain either the reference or the existing -32030 code.

**`meerkat-mobkit/src/http_console.rs:1758-1803`**

```text
(StatusCode::TOO_MANY_REQUESTS, "admission_backlog_full")
```

Read-only emits -32010/kind=read_only; the REST mapper emits backlog 429, probe/reload refusal 503, admission/reload timeout 504 and host-human-input 409, with error/message/data.

**`meerkat-mobkit/src/http_console.rs:3408-3455`**

```text
if state.decisions.console.read_only && is_console_mutating_rpc_method(&parsed_request.method) {
```

Multipart gates mutations before branching, but the agent.send ABAC check is inside console/send only. The main runtime HTTP dispatcher enforces its corresponding gates at 5445-5463.

## C-028: Historical v0.3/v0.4 source pointers name planning files absent from the checkout without archival qualification

**Severity:** low. **Decision:** rejected. **Disposition:** rejected; not applied.

### Original claim and proof

**`docs/rct/console-rest-sse-contract-v0.3.0.json:4-7`**

```text
"spec_source": {
    "rct_spec_markdown": ".rct/spec.md",
    "rct_spec_yaml": ".rct/spec.yaml"
  }
```

An auditor following the recorded source paths cannot resolve them and may incorrectly treat missing local planning artifacts as current required inputs.

**`docs/rct/console-rest-sse-contract-v0.4.0.json:4-7`**

```text
"rct_spec_markdown": ".rct/spec.md",
    "rct_spec_yaml": ".rct/spec.yaml"
```

Both earlier contracts contain the same unavailable source references. Reproducible read-only check from repository root: python3 -c 'from pathlib import Path; print(Path(".rct/spec.md").exists(), Path(".rct/spec.yaml").exists())' returned False False.

**`docs/rct/console-rest-sse-contract-v0.5.0.json:4-7`**

```text
"note": "This checked-in JSON file is the canonical contract source for v0.5.0; .rct planning files are not required inputs for this release gate."
```

The current contract already separates current authority from unavailable planning inputs; the historical entries lack that qualification.

### Independent adjudication

The file-absence observation is correct, but the proposed defect is not established. v0.3/v0.4 are explicitly version-pinned historical snapshots, and spec_source records their original planning provenance; it does not assert those inputs ship in today's checkout or remain a current gate. Absence now does not prove the historical provenance false. The active v0.5 contract already explicitly separates current authority from those planning inputs, and the current test selects v0.5. Treating an optional historical clarification as a necessary accuracy fix would exceed the evidence and risk rewriting provenance.

**`docs/rct/console-rest-sse-contract-v0.3.0.json:2-7`**

```text
"version_pin": "v0.3.0",
```

The questioned source paths are within an explicitly version-pinned historical artifact, not a current setup instruction.

**`docs/rct/console-rest-sse-contract-v0.4.0.json:2-7`**

```text
"version_pin": "v0.4.0",
```

The same historical framing applies to v0.4. A read-only Path.exists check did confirm that .rct/spec.md and .rct/spec.yaml are absent today.

**`docs/rct/console-rest-sse-contract-v0.5.0.json:4-7`**

```text
"note": "This checked-in JSON file is the canonical contract source for v0.5.0; .rct planning files are not required inputs for this release gate."
```

Current authority already expressly resolves the alleged ambiguity.

**`meerkat-mobkit/tests/console_route_auth.rs:277-285`**

```text
"../../docs/rct/console-rest-sse-contract-v0.5.0.json"
```

The current gate consumes v0.5, not the missing historical planning sources.

## C-029: WorkGraph's introduction incorrectly says every mutation requires expected_revision

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:917-920`**

```text
`expected_revision` is the compare-and-swap token on every
mutation.
```

The blanket normative statement conflicts with the actual method tables and encourages clients to require revision tokens for operations that have none.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:737-745`**

```text
let request: LinkWorkItemsRequest = parse_request(object)?;
```

Link is passed as a link request without MobKit revision parsing; the page's own link table correctly lists only kind/from_id/to_id/namespace.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:794-823`**

```text
let request: GoalCreateRequest = parse_request(object)?;
```

Creation is not a revision-checked update to an existing object. create, goal/create, link and terminal attention pruning are exceptions also visible in the documented parameter table.

**`meerkat-mobkit/tests/workgraph_rpc.rs:227-246`**

```text
"title": "ship the release",
            "description": "cut 0.7.30",
            "priority": "high",
            "labels": ["release"],
```

The end-to-end item lifecycle test creates successfully with these fields and no expected_revision, then obtains the revision for subsequent updates.

### Independent adjudication

The universal wording is contradicted by real dispatched operations and by the page's own method table. Creation and linking succeed without expected_revision; pruning also has a distinct typed request without MobKit revision parsing. Revision-checked updates remain genuinely mandatory and must not be weakened.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:737-743`**

```text
let request: LinkWorkItemsRequest = parse_request(object)?;
```

Link forwards its own typed request directly to the service, not a common mandatory revision wrapper.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:931-941`**

```text
let request: AttentionPruneRequest = parse_request(object)?;
```

Terminal-binding pruning is another separate mutation contract.

**`meerkat-mobkit/tests/workgraph_rpc.rs:231-246`**

```text
"title": "ship the release",
```

The create request supplies no expected_revision and yields a valid created item.

**`meerkat-mobkit/tests/workgraph_rpc.rs:341-347`**

```text
json!({ "kind": "related", "from_id": item_id, "to_id": second_id }),
```

A dispatched link request without expected_revision succeeds.

**`meerkat-mobkit/tests/workgraph_rpc.rs:436-443`**

```text
assert_eq!(error_code(&response), -32602);
```

The update-without-revision negative test proves the correction must remain operation-specific.

**Required correction:** Say expected_revision is the compare-and-swap token for revision-checked item/binding mutations, with required fields defined per method. Explicitly exclude item/goal creation, linking and terminal attention pruning from the blanket requirement.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Scoped expected_revision to revision-checked item/binding methods, with explicit creation/link/terminal-pruning exceptions and method-table authority.

**Validation:** Compared WorkGraph typed dispatch branches, mutation catalog and existing create/link/update-without-revision test source.

**Final review: pass.** The universal revision-token claim is now operation-specific, explicitly exempting item/goal creation, linking and terminal pruning while deferring required fields to the method table. It does not weaken genuinely revision-checked mutations.

**`docs/api/rpc.mdx:995-1001`**

```text
revision-checked item/binding mutations, as specified by each method below.
```

The following sentence names all adjudicated exceptions.

**`meerkat-mobkit/src/rpc/workgraph_methods.rs:737-743`**

```text
let request: LinkWorkItemsRequest = parse_request(object)?;
```

Link uses its typed request rather than a common expected_revision wrapper; pruning independently parses AttentionPruneRequest at 931-942. The create/link/update tests cited by adjudication remain consistent with this narrower statement.

## C-030: reload_member results may omit session_id and generation on a successful not_current response

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:397-398`**

```text
| `session_id` | `string` | The bound durable session after the reload (unchanged by a `discarded` reload) |
| `generation` | `integer` | The continuity generation after the reload (never advanced by a reload) |
```

Typed decoders following the table reject valid no-op reload responses for dormant/uninitialized identities.

**`meerkat-mobkit/src/identity_first/types.rs:1114-1118`**

```text
#[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<meerkat_core::types::SessionId>,
```

Both session_id and generation are optional and omitted when absent, not mandatory string/integer fields.

**`meerkat-mobkit/src/identity_first/runtime.rs:9255-9262`**

```text
disposition: MemberReloadDisposition::NotCurrent,
                session_id: None,
                generation: None,
```

An identity with no bound continuity record produces this successful no-op outcome.

**`meerkat-mobkit/src/rpc/mob_methods.rs:1280-1289`**

```text
let mut body = serde_json::to_value(&outcome).unwrap_or(Value::Null);
```

The dispatcher serializes the optional-field outcome directly and only adds identity/identity_first; it does not synthesize the missing fields.

### Independent adjudication

The outcome's fields are Option values with skip_serializing_if=None, and the dispatcher serializes that outcome directly. A no-binding NotCurrent path explicitly supplies None for both. The table must not require string/integer values for this valid success no-op. Existing reload tests cover no-op lifecycle semantics; the omission proof comes from the explicit branch plus serde attributes, not from claiming an unrun wire regression.

**`meerkat-mobkit/src/identity_first/types.rs:1114-1118`**

```text
pub session_id: Option<meerkat_core::types::SessionId>,
```

Both session_id and generation have skip_serializing_if=Option::is_none.

**`meerkat-mobkit/src/identity_first/runtime.rs:9256-9262`**

```text
session_id: None,
                generation: None,
```

No continuity record yields a successful not_current result with absent fields.

**`meerkat-mobkit/src/rpc/mob_methods.rs:1280-1288`**

```text
let mut body = serde_json::to_value(&outcome).unwrap_or(Value::Null);
```

The RPC does not fill in missing session/generation values; it only adds identity and identity_first.

**`meerkat-mobkit/tests/identity_first_runtime.rs:2717-2729`**

```text
meerkat_mobkit::identity_first::MemberReloadDisposition::NotCurrent
```

Existing tests affirm dormant reload is a success no-op rather than an error or automatic rematerialization.

**Required correction:** Mark session_id and generation optional/omitted when no continuity binding exists. Do not describe absence as null. Preserve the distinction between not_current no-ops and actual reloads, without expanding the session-preservation guarantee beyond the existing reload contract.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Marked reload session_id/generation optional in both the roster synopsis and detailed result table, omitted rather than null when no continuity binding exists.

**Validation:** Checked MemberReloadOutcome serde omission attributes, no-binding NotCurrent branch and direct RPC serialization.

**Final review: pass.** Both synopsis and detailed reload table now mark session_id/generation optional and specifically omitted, not null, when no continuity binding exists. The serialized success path does not fill absent fields.

**`docs/api/rpc.mdx:414-415`**

```text
omitted when no continuity binding exists
```

The synopsis at 345 uses session_id?/generation? consistently.

**`meerkat-mobkit/src/identity_first/types.rs:1114-1118`**

```text
#[serde(default, skip_serializing_if = "Option::is_none")]
```

Both fields have this omission attribute. runtime.rs:9256-9262 returns None/None for no binding; rpc/mob_methods.rs:1280-1288 serializes the outcome and only adds identity/identity_first.

## C-031: The WorkGraph realm is mob.<mob_id>, not the bare mob ID

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:929-931`**

```text
The `WorkGraphNamespaceGrant` meerkat requires per
agent is issued by MobKit from the wired service (realm = the mob id,
namespace `default`)
```

Hosts comparing realm IDs or inspecting persisted WorkGraph rows use the wrong key and can incorrectly conclude that records or grants belong to another realm.

**`meerkat-mobkit/src/workgraph_wiring.rs:107-123`**

```text
let realm = match meerkat_core::mob_realm_id(mob_id) {
```

The service is scoped through Meerkat's canonical mob-realm helper; source documentation at 91-93 identifies its spelling as mob.<mob_id>. install_workgraph_tools takes the namespace grant from this actual service.

**`meerkat-mobkit/tests/workgraph_rpc.rs:247-254`**

```text
json!(format!("mob.{}", runtime_mob_id(&runtime)))
```

The test asserts that a created item's serialized realm_id contains the mob. prefix, providing executable coverage rather than relying solely on a comment.

### Independent adjudication

The stock wiring calls mob_realm_id and grants the actual wired service scope. An independent literal assertion pins mob.wiring-realm, so this is not merely a comment repeated across files or a tautological test recomputing the helper. The raw mob-ID fallback is an error path that causes upstream member-tool validation refusal, not the valid standard grant spelling.

**`meerkat-mobkit/src/workgraph_wiring.rs:107-123`**

```text
let realm = match meerkat_core::mob_realm_id(mob_id) {
```

The service is constructed with the canonical mob realm and default namespace.

**`meerkat-mobkit/src/workgraph_wiring.rs:160-163`**

```text
Some(service.namespace_grant().clone()),
```

The automatically installed member grant is taken from that service rather than from the bare mob ID.

**`meerkat-mobkit/src/workgraph_wiring.rs:639-643`**

```text
assert_eq!(service.default_realm_id(), "mob.wiring-realm");
```

A literal test assertion independently pins the prefix.

**`meerkat-mobkit/tests/workgraph_rpc.rs:251-254`**

```text
json!(format!("mob.{}", runtime_mob_id(&runtime)))
```

The RPC integration test checks the same serialized item realm.

**Required correction:** Change the stock grant description to realm=Meerkat's canonical mob.<mob_id>, namespace=default. Preserve that callers cannot supply realm_id and that grants derive from the wired service.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Corrected the stock WorkGraph realm to canonical mob.<mob_id>, retaining default namespace and wired-service grant authority.

**Validation:** Checked workgraph_wiring.rs mob_realm_id/service grant and literal mob.wiring-realm test assertion.

**Final review: pass.** The stock realm spelling is corrected to mob.<mob_id>, while the default namespace, wired-service grant authority and rejection of caller-selected realms are preserved. The fallback error path is not presented as the normal realm format.

**`docs/api/rpc.mdx:1010-1015`**

```text
realm = Meerkat's canonical `mob.<mob_id>`
```

The surrounding namespace and scope authority qualifications remain intact.

**`meerkat-mobkit/src/workgraph_wiring.rs:107-123`**

```text
let realm = match meerkat_core::mob_realm_id(mob_id) {
```

The service uses the canonical realm; lines 160-163 copy its actual namespace grant. The independent literal test at 643 pins mob.wiring-realm.

## C-032: memory/query is not a backend-availability check and does not return -32012

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/api/rpc.mdx:132-132`**

```text
| `-32012` | `memory/{index,query}` backend unavailable | none |
```

Clients rely on query's alleged backend-unavailable error to detect storage loss, even though a successful cached query proves nothing about persistence availability.

**`meerkat-mobkit/src/rpc.rs:2710-2720`**

```text
let query_result = runtime.memory_query(query_request).await;
```

A valid query is infallible here and always serialized as a successful result; no backend-unavailable branch exists. The module-only dispatcher has the same behavior.

**`meerkat-mobkit/src/runtime/memory.rs:360-365`**

```text
pub fn memory_query(&self, request: MemoryQueryRequest) -> MemoryQueryResult {
```

Query filters the in-memory assertion/conflict collections, not the persistence backend. By contrast, index maps BackendPersistFailed to -32012 in rpc.rs:2672-2682.

### Independent adjudication

The symbol's adjacent Rust documentation also mentions query, but executable dispatch is decisive: valid queries return the in-memory result with no backend-availability branch. Index alone maps BackendPersistFailed to -32012. I checked both module and unified paths rather than treating the constant comment as proof of runtime behavior.

**`meerkat-mobkit/src/rpc.rs:2710-2728`**

```text
let query_result = runtime.memory_query(query_request).await;
```

The unified query arm serializes success; its only explicit error branch is invalid params (-32602). The module arm at 1011 has equivalent behavior.

**`meerkat-mobkit/src/runtime/memory.rs:360-415`**

```text
pub fn memory_query(&self, request: MemoryQueryRequest) -> MemoryQueryResult {
```

The return type is infallible and filters loaded memory_assertions/memory_conflicts rather than consulting a persistence backend.

**`meerkat-mobkit/src/rpc.rs:2672-2682`**

```text
Err(MemoryIndexError::BackendPersistFailed(error)) => JsonRpcResponse {
```

The -32012 mapping is specifically in memory/index.

**`meerkat-mobkit/tests/memory_store.rs:618-625`**

```text
indexed["error"]["code"],
```

The existing backend-failure regression asserts -32012 on the index response, not a query health check.

**Required correction:** Limit the -32012 table entry to memory/index backend-persistence failure. State that memory/query reads the loaded in-memory ledger and does not test backend availability. Do not alter runtime behavior or unrelated Rust documentation in this scope.

### Changes and final verification

**Changed:** `docs/api/rpc.mdx`.

Restricted -32012 to memory/index persistence failures and explicitly stated memory/query only filters the loaded ledger, not backend health.

**Validation:** Checked both RPC index/query dispatch and runtime/memory.rs infallible loaded-ledger query implementation.

**Final review: pass.** The error table now limits -32012 to memory/index persistence failure, and the query section explicitly states loaded-ledger reads are not backend-health probes. Both module and unified dispatch support that distinction.

**`docs/api/rpc.mdx:133-134`**

```text
`memory/index` backend persistence failed
```

The query introduction at 766-769 expressly excludes -32012 backend-unavailability behavior.

**`meerkat-mobkit/src/rpc.rs:2672-2678`**

```text
Err(MemoryIndexError::BackendPersistFailed(error)) => JsonRpcResponse {
```

This is the -32012 branch. Unified query at 2710-2728 and module query at 1011-1029 instead serialize an infallible runtime.memory_query result, with only invalid-params errors.

## Independent scope checks

> [
>   "Read the wave-1 ledger and independently adjudicated all 32 IDs against local source; checked actual HTTP and stdio dispatch distinctions rather than relying on the skill summary or the earlier audit.",
>   "Read relevant existing gating_policy, routing_delivery, memory_store, access_control, workgraph_rpc, identity_first_runtime and console_route_auth tests, plus inline HTTP/access/cross-mob/replay unit tests. Tests are cited as checked source, not reported as executed.",
>   "Executed read-only Python tomllib parsing of the access-control guide's complete first TOML block: the only undefined referenced group is contractors.",
>   "Executed read-only JSON parsing of all five docs/rct console contract files: all parsed and declared their filename versions.",
>   "Executed read-only source searches: no mobkit/rediscover, mobkit/reconcile_identity or mobkit/reconcile literal in http_console.rs; reconcile_edges is present; with_read_deadline is absent from http_console.rs. Followed both HTTP dispatch fallbacks and the member-declaration registry to rule out generic forwarding.",
>   "Confirmed .rct/spec.md and .rct/spec.yaml are absent, but rejected C-028 because absence is not proof of false historical provenance or a current required-input claim.",
>   "Verified HEAD is af82b6b3ab34faed9bf3e962d148d55f10dcd1dc and repository status was clean before and after writing this out-of-repository adjudication artifact.",
>   "Validated adjudication JSON, unique one-to-one coverage of all 32 audit IDs, correction/verdict consistency, and exact substring matching of all 120 evidence quotations inside their cited source line ranges."
> ]

## Final scope checks

> [
>   "Read the audit brief and scope manifest, audit-C/adjudication-C/fixes-C, and applicable audit-K/adjudication-K findings. Independently inspected every confirmed correction and the complete owned-document diff. No repository edits, nested delegation, commits or git-state mutations were performed by this reviewer.",
>   "Report coverage validation: exactly 31 confirmed C IDs plus K-001/K-005; C-028 is the sole rejected C finding and is not counted as a fix.",
>   "Byte comparisons against baseline af82b6b3ab34faed9bf3e962d148d55f10dcd1dc passed for v0.1.0, v0.2.0, v0.3.0 and v0.4.0, including the rejected C-028 planning-provenance fields. The authentication guide is also byte-identical to baseline.",
>   "Parsed all five docs/rct JSON files. Independently asserted current send required/optional sets, exact blob result keys and selectors, limit-only delivery history, conditional HTTP approver and optional anonymous-preview subject. Every policy_error_refs JSON pointer resolves.",
>   "Compared all 50 canonical RPC entries' read-only applicability with the actual HTTP mutation classifier and delegated WorkGraph/topology catalogs, resolving the topology method constant. Applicable ABAC inventories contain shared references or existing -32030 entries; distinguished post-load record authorization from upfront gates, filtered reads and ungated standalone blob upload.",
>   "Exact response-key comparisons against the actual Rust structs passed for the gating/evaluate and routing/resolve concrete JSON examples. Independently checked identity/inspection, history, reload optional-field, multipart result and timeline-envelope shapes against their serializers.",
>   "Parsed the complete introductory access TOML with tomllib and verified every referenced group exists. Checked balanced Markdown fences and all local page/heading links in C-owned MDX files.",
>   "Parsed all eight concrete SSE data examples as JSON. The pre-existing structural-event illustration containing an unquoted ellipsis is schematic and was excluded after an initial overly broad JSON check; it is unchanged, not a regression. Corrected the verification script's initial omission of the topology constant and its assumption that record-detail authorization was upfront; final assertions passed.",
>   "git diff --check passed for docs/api, docs/concepts/access-control.mdx, docs/concepts/events.mdx, docs/rct and docs/guides/authentication.mdx.",
>   "./scripts/repo-cargo test --locked --quiet -p meerkat-mobkit --test console_route_auth phase0_contract_004_console_rest_sse_contract_version_is_pinned_and_enforced -- --exact: PASS, 1 passed, 0 failed, 6 filtered out. Only the non-fatal existing large __eh_frame linker warning appeared.",
>   "Read console_route_auth.rs:277-397 to bound that test's evidence: it pins v0.5, checks experience/modules and selected timeline/error metadata; it does not exhaustively validate all changed schemas. The independent parser/dispatcher/serializer checks above supply the additional evidence. No live gateway/provider or broad runtime-suite execution is claimed.",
>   "Cross-scope K-005 guide occurrences remain outside this review's ownership; this verdict covers the required C-owned RPC occurrence. Historical contracts remain intact.",
>   "2026-09-22 C-001 residual recheck: read fixes-review-residuals.json B-007 and independently verified that matching rpc_gateway SDK/stdio policy fills omitted risk_tier and overrides supplied values before the required-tier parser. Refreshed only this review artifact's C-001 evidence/reason, recorded the feedback history, and revalidated its exact quotation ranges and unchanged seven-key result example. Existing test history is retained; no documentation, runtime or SDK files were edited and no new Rust-test execution is claimed."
> ]
