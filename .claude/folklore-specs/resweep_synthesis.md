All verified exactly as described. The claims are accurate against the actual code. I have enough verified ground truth across the spectrum (auth, connection, workgraph, mob, mcp, contracts, llm-core) to trust the rest of the panel's site-level descriptions and proceed to clustering.

One observation worth noting for the report: the input itself contains contradictions on the mob-mcp wire-action item — it appears twice (line 3500 at `low` and line 3137 at `medium`) for the same root carrier. I'll fold those into one cluster member. Same for the four auth_method sites which are all one oracle.

Here is the synthesis.

---

# Rule-4 Folklore Re-Sweep — Synthesis Report

## Scope note

29 confirmed-new violation records were supplied. Several are duplicate facets of one carrier (e.g. the mob-mcp wire-action appears as both `lib.rs:3500` and `lib.rs:3137`; the four `auth_method` sites are one oracle). I verified 14 anchor sites against live code before clustering — all matched the panel's descriptions exactly, including the load-bearing workgraph trust-bypass and the `confirmation_kind` doc comment that confirms `kind: String` is meant to be opaque. The clustering below collapses the 29 records into **10 root-shape clusters** plus contested-site verdicts.

---

## Cluster N1 — Auth-method String Oracle (the persisted-auth-mode table run off `auth_profile.auth_method: String`)

**Title:** A typed per-provider auth-method enum (`NormalizedAuthMethod`) already sits on `ValidatedBinding`, but every credential decision re-derives `PersistedAuthMode` from the raw `auth_method` String via the one string-keyed table `persisted_auth_mode_for_auth_method(&str)`.

**Member sites (one carrier, many consumers):**
- `meerkat-auth-core/src/oauth_flow.rs:337` — `validate_oauth_target_for_auth_mode` (gates whether an OAuth flow may store credentials) — **high**
- `meerkat-auth-core/src/auth_store/mod.rs:37` — the string-keyed table itself (`pub`, crosses crate via providers shim) — **high**
- `meerkat-auth-core/src/resolver.rs:342` — OAuth-login lifecycle-guard acquisition — **medium**
- `meerkat-auth-core/src/resolver.rs:894` — `require_persisted_auth_mode` persisted-mode mismatch rejection + stale-credential errors — **medium**
- `meerkat-rpc/src/handlers/auth.rs:913` — `handle_auth_profile_create`: 2-literal allowlist (`api_key`/`static_bearer`) diverging from the canonical 7-method table — **medium** (carries a real latent bug: 5 valid direct-secret methods are silently unreachable via RPC)
- `meerkat-rest/src/auth_endpoints.rs:795` — identical 2-arm divergent subset at the REST create path — **medium**

**Typed owner:** EXISTS — `NormalizedAuthMethod` (`meerkat-llm-core/src/provider_runtime/binding.rs:39`, carried on `ValidatedBinding.auth`, exposed via `.auth()`), backed by per-provider `{OpenAi,Anthropic,Google,SelfHosted}AuthMethod` in `meerkat-core/src/provider_matrix/`. Target enum `PersistedAuthMode` exists (`meerkat-core/src/auth/token_store.rs:112`). The OAuth-login predicate `persisted_auth_mode_uses_oauth_login_lifecycle` exists (`auth/lifecycle.rs:73`). **MISSING:** a typed `NormalizedAuthMethod -> Option<PersistedAuthMode>` mapping method.

**Impact:** wire ✓ (`oauth_flow`, REST/RPC create) · durable-metadata ✓ (writes `PersistedTokens.auth_mode`) · generated-machine ✗
**Severity (cluster):** HIGH (two high-severity members + a live functional bug at the RPC/REST create surfaces).

**Ordered repair:**
1. Add `NormalizedAuthMethod::persisted_auth_mode(self) -> Option<PersistedAuthMode>` in meerkat-core, delegating to each per-provider `*AuthMethod` (e.g. `OpenAiAuthMethod::ManagedChatGptOauth -> ChatgptOauth`), mirroring the existing `OAuthFlowTarget::auth_mode()` precedent.
2. Add a createability predicate (`!persisted_auth_mode_uses_oauth_login_lifecycle(mode)`) colocated with the owner so "direct-secret only at create surfaces" is expressed once over the typed enum.
3. Switch resolver (`342/356/436/894`) and `oauth_flow.rs:337` to read `binding.auth()` (the typed value already in scope) instead of `binding.auth_profile().auth_method`.
4. Fix the RPC (`handlers/auth.rs:913`) and REST (`auth_endpoints.rs:795`) create paths to delegate to the canonical table + createability predicate — this also closes the 5-method gap. Make `AuthMethodMismatch` carry the typed enum.
5. Reduce `persisted_auth_mode_for_auth_method(&str)` to a thin parse-at-boundary shim for the genuine string-edge callers (CLI/REST/RPC inbound), so the semantic table lives once on the type.

---

## Cluster N2 — Provider identity recovered from `&'static str` (the `LlmClient::provider()` outlier)

**Title:** `LlmClient::provider() -> &'static str` is the untyped identity outlier in a crate where `ProviderRuntime::provider_id()` / `ValidatedBinding::provider()` already return the typed `Provider`; shared adapter/factory logic re-derives `Provider` from that text.

**Member sites:**
- `meerkat-llm-core/src/types.rs:60` — the trait method declaration (root carrier) — **medium**
- `meerkat-llm-core/src/adapter.rs:119` — `apply_generic_provider_overrides` matches `"anthropic"` / `"gemini" | "google"` to pick a `ProviderTag` variant; `_ => tag` fails OPEN (drops the override silently); dead `"google"` arm no impl returns — **medium**
- `meerkat/src/factory.rs:2634/2637` — `Provider::from_name(client.provider())` round-trips the string straight back into the enum

**Typed owner:** EXISTS — `Provider` enum (`meerkat-core/src/provider.rs:9`) with `parse_strict`/`from_name`/`as_str`; the factory at `factory.rs:3746` already branches on the typed `Provider` before discarding it and handing the adapter only `Arc<dyn LlmClient>`.

**Impact:** wire ✗ · durable ✗ · generated-machine ✗ · **but fails open** (override dropped on unrecognized/mistyped provider string)
**Severity (cluster):** MEDIUM.

**Ordered repair:**
1. Change `LlmClient::provider()` (and the delegating `AgentLlmClient::provider()`) to return `Provider` instead of `&'static str`; provider impls return the enum (each statically knows its identity). *Or* thread `provider: Provider` into `LlmClientAdapter::new` from `factory.rs:3746` where it is already typed.
2. Rewrite `adapter.rs:119` as `match self.provider { Provider::Anthropic => …, Provider::Gemini => …, Provider::OpenAI|SelfHosted|Other => tag }` — collapsing the `"gemini" | "google"` alias into one variant and making `OpenAI`/`Other`/`SelfHosted` explicit no-op arms rather than a silent fall-through.
3. Drop the `Provider::from_name(...)` round-trip at `factory.rs:2634/2637`.
4. Keep a separate `provider_label() -> &'static str` (or `.as_str()`) for the legitimate error/log sites (`adapter 195/342/351/383`, `AgentError::llm`).

---

## Cluster N3 — Synthetic env-default binding recovered from the magic slug `"env_default"`/`"default"`

**Title:** "This auth binding is the synthetic env-var fallback (ephemeral, not durable)" is recovered by string equality on `RealmId`/`BindingId` inner slugs rather than a typed origin discriminant; it fans out into durable-persistence decisions across core + facade.

**Member sites:**
- `meerkat-core/src/connection.rs:137` — `is_env_default()` (`realm == "env_default" && binding == "default" && profile.is_none()`) — root reader
- `meerkat-core/src/connection.rs:446` — short-circuits omitted-binding resolution to `UnknownRealm`
- `meerkat-core/src/connection.rs:851` — `synthesize_default_from_spec` hardcodes the magic slug (producer)
- `meerkat/src/factory.rs:1684/1708` — `selected_realm == "env_default"` skip-configured-lookup; `1804/1819/2340/2501-2511/3437-3443/3622/3637` — `is_env_default()` gates whether a credential lease identity is **persisted as durable metadata** vs `None` (recovery/eligibility branches). Test `explicit_env_default_auth_binding_is_not_rehydrated_as_durable_identity` (factory.rs:5486) enforces the behavior.

**Typed owner:** NEW — no synthetic/origin field exists today. Add `enum BindingOrigin { Configured, SyntheticEnvDefault }` carried on `ResolvedConnectionTarget` (and on `AuthBindingRef` if it must survive the round-trip into the factory). Stamp it authoritatively at the two construction seams (`synthesize_default_from_spec => SyntheticEnvDefault`; configured materialize => `Configured`).

**Impact:** wire ✓ (`AuthBindingRef` is Serialize/Deserialize; lease identity is persisted) · **durable-metadata ✓** (governs lease publishing) · generated-machine ✗
**Severity (cluster):** MEDIUM.

**Ordered repair:**
1. Add `BindingOrigin` in `connection.rs`.
2. Stamp at the two seams. `is_env_default()` becomes `matches!(self.origin, SyntheticEnvDefault)`.
3. Replace all `connection.rs:446` + factory guards (1684/1708/1804/1819/2340/2501/3437/3622/3637) with variant matches.
4. Keep `RealmId`/`BindingId` as pure opaque slug newtypes; the synthetic case no longer mints the magic slug.
5. If origin must survive persistence, serialize the discriminant (defaulting to `Configured` is acceptable pre-1.0). Update the `realm_id == "env_default"` assertion tests to assert the variant.

---

## Cluster N4 — Per-provider default binding recovered from the `default_<provider>` name convention

**Title:** "This binding is the default for provider X" is carried only by the `default_<provider>` name; the realm-level typed `default_binding: Option<String>` expresses only a *single* per-realm default and cannot express a per-provider default, so the system falls back to the string convention.

**Member sites:**
- `meerkat-core/src/connection.rs:516` — `selected_binding_id_for_provider` synthesizes `format!("default_{provider}")` and string-matches existing BindingIds (consumer)
- `meerkat-core/src/connection.rs:984` / `:1021-1022` — `from_inline_api_keys` mints `default_{provider}` ids but sets the typed `default_binding` only for `idx==0`; for 2nd+ providers default-ness is encoded *only* by the name (producer)
- `meerkat-web-runtime/src/lib.rs:314` — independently re-synthesizes `format!("default_{provider}")` to look up + mutate a backend `base_url` across the crate boundary

**Typed owner:** NEW — a per-binding default-role marker, e.g. `ProviderBindingConfig.provider_default: bool` (or a `BindingPolicy`-carried `DefaultRole`). The realm-level `default_binding` is the single-default owner; it does **not** cover the per-provider case.

**Impact:** wire ✓ · durable ✗ · generated-machine ✗
**Severity (cluster):** MEDIUM.

**Ordered repair:**
1. Add the typed per-binding default marker.
2. In `from_inline_api_keys`, set it per provider (keep `idx==0` feeding the realm default if a single realm-default is still wanted).
3. In `selected_binding_id_for_provider`, scan `provider_bindings` for the typed marker instead of synthesizing the name.
4. In `web-runtime/lib.rs:312-318`, look up by typed marker / explicit returned id rather than re-deriving the name. `BindingId` becomes pure opaque identity.

> **Cross-cluster note:** N3, N4, and the contested `connection.rs:571` (refutation upheld below) all live in `connection.rs`'s binding-resolution path. Sequencing them together avoids three independent edits to the same resolver.

---

## Cluster N5 — Typed enum laundered through its own variant-name text (typed → `.as_str()` → typed round-trips)

**Title:** A value that is *already* a typed enum is flattened to a `&str`/`String`, ferried, then re-parsed into a (sometimes identical) typed enum, branching on the result — with a dead error arm that can never fire because the source is a closed type.

**Member sites:**
- `meerkat-mob/src/run.rs:4544` — `step_status_snapshot` calls generated `flow_run::StepRunStatus::as_str()` then `StepRunStatus::from_flow_run_status(text)` to rebuild the mob crate's *identical* `StepRunStatus`; dead `other => Err(Internal)` arm. (Same root cause as confirmed cluster M8, distinct site.) **generated-machine ✓** — **medium**
- `meerkat-openai/src/live.rs:3825` — typed `Provider` → `as_str().to_string()` at two producers → compared by `String !=` across the shared-core projection boundary to reject a provider swap; **fails OPEN** on drift. **wire ✓** — **medium**
- `meerkat-cli/src/main.rs:7237` — typed `ExternalToolDeltaPhase::Failed` → `status_text()` display string → `.starts_with("failed")` to decide whether to emit a failure warning — **medium**

**Typed owner:** EXISTS in all three — `flow_run::StepRunStatus` + the existing typed converter `step_status_from_flow_run` (`runtime/flow.rs:1957`); `Provider` enum + the sibling typed `LiveConfigRejectionReason::ChannelIdentitySwap{from_provider, to_provider}`; `ExternalToolDeltaPhase` (`event.rs:1247`, field at `event.rs:1366`).

**Impact:** wire ✓ (live.rs) · generated-machine ✓ (run.rs) · durable ✗
**Severity (cluster):** MEDIUM. (live.rs and run.rs fail open / carry dead arms; CLI is fragile to wording changes.)

**Ordered repair:**
1. `run.rs:4544`: replace the `from_flow_run_status(value.as_str())` call with a direct `flow_run::StepRunStatus -> StepRunStatus` match (reuse/relocate `step_status_from_flow_run`, make it `pub(crate)`); delete the dead text-matcher at `run.rs:5545`. Sweep the M8 siblings (`4466`, `4509`) the same way.
2. `live.rs`: retype `LiveProjectionSnapshot.provider_id` (`live_adapter.rs:622`) and `OpenAiRealtimeSession.current_provider_id` (`live.rs:1269`) to `Provider`; stamp directly (drop `.as_str().to_string()` at `2809/2852` and `.as_str()` at `live_orchestration.rs:145`); make `RefreshProviderSwap{from_provider,to_provider}` carry `Provider`. The `3825` guard becomes `Provider == Provider`.
3. `main.rs:7237`: replace `status_text().starts_with("failed")` with `notice.phase == ExternalToolDeltaPhase::Failed`; keep `status_text()` only as the human-readable detail.

---

## Cluster N6 — Two-variant operation discriminator as untyped `action: String` at the mob-MCP tool surface

**Title:** The wire/unwire operation is round-tripped through `WireActionArgs.action: String` (re-matched in two places) at an agent-tool boundary, while the sibling `mob_lifecycle` tool in the *same file* already uses the typed `WireMobLifecycleAction` enum explicitly built to kill `action: String` folklore.

**Member sites (one carrier; the two input records `lib.rs:3500` + `lib.rs:3137` are the same violation):**
- `meerkat-mob-mcp/src/lib.rs:3137` — `WireActionArgs.action: String`
- `meerkat-mob-mcp/src/lib.rs:3218` — secondary `action == "wire"` guard gating external-binding requirements
- `meerkat-mob-mcp/src/lib.rs:3500` — `match action.as_str()` dispatch to `mob_wire`/`mob_unwire` with a runtime catch-all

**Typed owner:** NEW — add `WireMobWireAction { Wire, Unwire }` (`#[serde(rename_all="snake_case")]`) to `meerkat-contracts/src/wire/mob.rs`, sibling to `WireMobLifecycleAction` (mob.rs:1539, whose doc explicitly says it replaced an `action: String` discriminator).

**Impact:** wire ✓ (JSON schema already declares `enum:["wire","unwire"]`, so the wire surface is unchanged) · durable ✗ · generated-machine ✗
**Severity (cluster):** MEDIUM (the higher of the two duplicate judgments; it is a typed-contract gap on an agent-facing boundary, even if it fails closed at runtime).

**Ordered repair:**
1. Add `WireMobWireAction` to `wire/mob.rs`.
2. `WireActionArgs.action: String -> WireMobWireAction`; `resolve()` returns the typed enum; replace `action == "wire"` with `matches!(action, Wire)`.
3. Replace `match action.as_str()` with an exhaustive typed match; delete the now-impossible catch-all.
4. Update the 3 test fixtures (`lib.rs:4128/4145/4167`).

---

## Cluster N7 — Surface DTO error/status classes collapsed to `String` and re-derived at the SDK boundary

**Title:** Closed typed error/status domains are serialized to untyped `String` (or `Vec<Value>`) on outbound DTOs, then re-derived into branch decisions at SDK surfaces.

**Member sites:**
- `meerkat-mcp-server/src/lib.rs:2066` — `WorkGraphToolError.code: String` (collapsed from the 9-variant `WorkGraphError`); surface `map_workgraph_tool_error` re-matches `error.code.as_str()` to the `WORKGRAPH_TOOL_*` constants → JSON-RPC numeric code, with a silent `_ => -32603`. Two arms already use `meerkat_contracts::ErrorCode::jsonrpc_code()`, proving the typed pattern. **wire ✓** — **medium**
- `meerkat-contracts/src/wire/mob.rs:1769` — `MobSnapshotResult.status` / `MobStatusResult.status` (593) / `MobRespawnResult.status` (864) / `MobAppendSystemContextResult.status` (1587) collapse typed domains to `String`; `MobSnapshotResult.members: Vec<Value>` collapses typed `MobMemberListEntry`. The load-bearing re-derivation: Web SDK `parseMobRespawnResult` (`sdks/web/src/mob.ts:656`) branches `status !== 'completed' && status !== 'topology_restore_failed'`. **wire ✓** — **low** (only the respawn-outcome domain is currently decision-bearing; lifecycle status is display-only). *2/3 refuter consensus.*

**Typed owner:** mixed — `WireMobMemberStatus`, `MobMemberListEntryWire`, and core `AppendSystemContextStatus` already exist. **MISSING/NEW:** `WorkGraphToolErrorCode` (derived from `WorkGraphError`); `WireMobLifecycleStatus` (mirror of `MobState`); `WireMobRespawnOutcome` (mirror of `MobRespawnError::TopologyRestoreFailed` vs success).

**Impact:** wire ✓ (regen-schemas + version-parity) · durable ✗ · generated-machine ✗
**Severity (cluster):** MEDIUM (driven by the workgraph member; the mob-result member is low).

**Ordered repair:**
1. `WorkGraphToolError.code: String -> WorkGraphToolErrorCode` built directly from `WorkGraphError` in `map_error()`; add `jsonrpc_code()` (reuse `ErrorCode` where aligned); delete the surface string match and the `_ => -32603` fall-through.
2. Add `WireMobLifecycleStatus` / `WireMobRespawnOutcome`; replace the four `status: String` fields; map producers from the typed source enum; replace `MobSnapshotResult.members: Vec<Value>` with `Vec<MobMemberListEntryWire>`.
3. Web SDK `parseMobRespawnResult` validates the tagged enum.
4. `make regen-schemas` + verify-version-parity.

---

## Cluster N8 — Undocumented String / `Value["kind"]` sentinels crossing wire/surface boundaries to drive a binary policy

**Title:** A real two/three-way policy distinction is reconstructed by string-matching a sentinel on a wire field (or a raw `serde_json::Value` tag), where a typed enum either exists downstream or should own the distinction.

**Member sites:**
- `meerkat-session/src/persistent.rs:5451` — `query.revision == "current"` selects resolve-to-head vs literal-revision. The undocumented wire sentinel `"current"` flows from contracts `ReadSessionTranscriptRevisionParams.revision: String` → RPC router → `SessionTranscriptRevisionQuery.revision`; Python/TS SDKs pass `revision: str`. **wire ✓** — **medium**
- `meerkat-contracts/src/request_lifecycle.rs:40` — `session_create_runs_immediately` parses raw `Value` and matches `value["initial_turn"].as_str() == Some("deferred")` to pick `LongRunningPublishOnSuccess` vs `InlineObservation` (gates the long-running executor). A typed owner exists downstream (`InitialTurn` in rpc, `InitialTurnPolicy` in core) but is unreachable from contracts → it is a *second divergent copy* of the same wire contract. **wire ✓** — **medium**
- `meerkat-schedule/src/tools.rs:434` — `rewrite_current_session_target_object` matches `target["target_kind"]=="session" && target["type"]=="current_session"` on raw `Value` then mutates it; `"current_session"` is a synthetic string-only pseudo-variant the typed `SessionTargetBinding` enum cannot represent. **medium**

**Typed owner:** mixed — `SessionTranscriptRevisionQuery` exists (retype its field); `InitialTurnPolicy` exists in core but lacks `Deserialize`; `SessionTargetBinding` exists but lacks a `CurrentSession` arm. NEW pieces: `RevisionSelector { Current, Specific(RevisionId) }`; a contracts-reachable `WireInitialTurn` (or `Deserialize` on `InitialTurnPolicy`); `SessionTargetBinding::CurrentSession { action }`.

**Impact:** wire ✓ (revision + initial_turn cross the wire) · durable ✗ · generated-machine ✗
**Severity (cluster):** MEDIUM.

**Ordered repair:**
1. `persistent.rs`: introduce `RevisionSelector { Current, Specific(RevisionId) }`, parse `"current"` once at the wire/surface seam (`ReadSessionTranscriptRevisionParams`), match the enum in `read_transcript_revision`. (Minimal: keep the wire String, retype only the internal query.) Optional `RevisionId(String)` newtype for the `sha256:` digests.
2. `request_lifecycle.rs`: define a deserializable `WireInitialTurn { RunImmediately, Deferred }` in contracts (or add `Deserialize` to `InitialTurnPolicy`), deserialize a typed view instead of `Value.as_str()`, keep the same fail-open default. Fold the divergent rpc `InitialTurn` copy into one owner.
3. `schedule/tools.rs`: add `SessionTargetBinding::CurrentSession { action }`; deserialize into the typed enum and rewrite to `ResumableSession` via a typed constructor instead of poking `Value`; drop the manual schema string push.

---

## Cluster N9 — Auth/connection error outcomes classified by error-message substring instead of typed status

**Title:** Re-auth-vs-retry (and OAuth-suggestion) decisions are driven by `.contains()` on a flattened HTTP/error string, bypassing typed `StatusCode` / parsed RFC-6749 `error` codes / the canonical `RefreshFailureObservation` the AuthMachine already consumes.

**Member sites:**
- `meerkat-auth-core/src/mcp_oauth.rs:606` — `map_refresh_error`: `body.contains("invalid_grant") || body.contains("invalid refresh")` → `ReauthRequired` vs `RefreshFailed`. Bypasses `oauth_token_endpoint_error_code` / `oauth_refresh_observation` / the generated AuthMachine reauth guards; the degraded subset false-positives on `invalid_grant_foo`, misses `invalid_client`/`access_denied`/401/403, and invents a non-canonical `"invalid refresh"` synonym. **generated-machine ✓** — **medium**
- `meerkat-mcp/src/connection.rs:421` — `auth_failure_suggests_oauth` substring-matches `"401"/"403"/"Unauthorized"/"Forbidden"/"Auth required"` to drive interactive OAuth login. The transport (`streamable_http.rs`) has typed `StatusCode::UNAUTHORIZED|FORBIDDEN` and `StreamableHttpError::AuthRequired`, swallowed by `().serve()` and flattened into `reason`. The `auth_challenge.is_some()` branch is the correct typed path; the four `.contains()` fallbacks are the violation. **medium**

**Typed owner:** EXISTS for mcp_oauth (`oauth_refresh_observation` → `RefreshFailureObservation`; the AuthMachine reauth predicate is the canonical classifier). NEW for mcp connection — plumb a typed connect-failure outcome through `().serve()`: widen `AuthChallengeRecorder` to record `StatusCode` on every 401/403 and surface `StreamableConnectError.auth_status: Option<StatusCode>`, or add `enum ConnectFailureKind { Auth { challenge }, Other }`.

**Impact:** wire ✗ · durable ✗ · generated-machine ✓ (mcp_oauth must consult the same predicate the AuthMachine uses)
**Severity (cluster):** MEDIUM.

**Ordered repair:**
1. `mcp_oauth.rs`: delete the substring match; route the `OAuthError` through `oauth_refresh_observation` to get a `RefreshFailureObservation`; expose the AuthMachine's reauth predicate as `RefreshFailureObservation::requires_reauth()` so MCP and lease lifecycle share one owner; drop the `"invalid refresh"` synonym. (Existing JSON-body tests pass unchanged.)
2. `mcp/connection.rs`: capture `StatusCode` typed at the transport seam (`AuthChallengeRecorder::record`), surface it on `StreamableConnectError`; rewrite `auth_failure_suggests_oauth` to `auth_challenge.is_some() || auth_status.is_some()`; delete all five `.contains()` checks.

---

## Cluster N10 — Hand-rolled enum encoded as `&[&str]` / `&str` literals re-classified by `.contains()`/`==`

**Title:** A small closed vocabulary (wiring sides; flow-reference roots) is carried as string literals and re-derived by string comparison in runtime logic, where a typed enum/AST should exist.

**Member sites:**
- `meerkat-mob/src/runtime/actor.rs:9643` (+ producers 9454/9620/9762/9876/10936/10961, empty-slice 9690/9725; consumers 10583/10599/10637/10654) — `installed_sides: &[&str]` of `"local"`/`"peer"`, re-classified via `.contains(&"local")`/`.contains(&"peer")` to drive wiring-rollback compensation; empty `&[]` silently means "no sides." Private, never serialized, best-effort. **low**
- `meerkat-mob/src/runtime/path.rs:20` — `resolve_context_path` splits a flow-reference expression on `.` and matches the leading segment (`"params"/"steps"/"loops"` + `"iterations"`/`"output"`) to recover the reference root-kind and dispatch to a `FlowContext` map. Drives `evaluate_condition` (loop-until / step guards) and template rendering — both shared mob runtime. The residual deep `walk_json` into unbounded step-output JSON is legitimately stringy; the **root selector** is a typeable closed enum. **wire ✓ · durable ✓** — **medium**

**Typed owner:** NEW in both — `WiringSides` flag set (`Local | Peer`, with `local()/peer()/both()/empty()/has_local()/has_peer()`) local to `actor.rs`; a `FlowRef { root: FlowRefRoot, json_path: Vec<String> }` AST with `FlowRefRoot::{Params, Step(StepId), LoopIteration{loop_id, iteration, step_id}}` parsed once at the flow-definition boundary.

**Impact:** actor.rs — none (wire/durable/machine all ✗); path.rs — wire ✓, durable ✓
**Severity (cluster):** MEDIUM (driven by `path.rs`; `actor.rs` is low).

**Ordered repair:**
1. `actor.rs`: introduce `WiringSides`; change both rollback helper signatures from `&[&str]` to `WiringSides`; replace `.contains(...)` with `has_local()/has_peer()` and all producer literals with constructors; `&[]` becomes `WiringSides::empty()`.
2. `path.rs`: introduce `FlowRef` + `FlowRef::parse(&str) -> Result<FlowRef, FlowRefParseError>` doing the split/classification/iteration parsing once; make `ConditionExpr.path` (and template placeholders) deserialize/validate into `FlowRef` at definition; `resolve_context_path` matches on `ref.root` then `walk_json(output, ref.json_path)` for the unavoidable dynamic leaf walk; keep fail-closed (parse error → condition false / Null render).

---

## Singletons (genuine violations not sharing a root shape with the clusters above)

These survived the panel but are structurally distinct; folding them into a cluster would be artificial.

| ID | Site | Shape | Sev | Owner | Impact |
|----|------|-------|-----|-------|--------|
| **S1** | `meerkat-core/src/surface_metadata.rs:100` (+ `:107`) | Meerkat-owned-vs-caller key-space boundary as `key.starts_with("meerkat.")` + literal `RESERVED_MOB_LABEL_KEYS`; drives `ReservedLabelKey`/`ReservedAppContextKey` rejection at the public boundary (`surface.rs:417/422`). | medium | NEW `ReservedMetadataKey::classify(key)`; fold with existing `ReservedSessionMetadataKey`/`is_session_authority_metadata_key` (`session.rs:792/804`) into one reserved-key authority. | durable ✓. *2/3 refuter consensus — slightly thinner.* |
| **S2** | `meerkat-core/src/types.rs:1155` | `SystemNoticePeer.id: String` where typed `comms::PeerId` exists; consumer re-parses via `PeerId::parse` at `types.rs:1493` and branches Ok/Err to pick canonical vs degraded projection. | medium | EXISTS — `comms::PeerId` (`comms.rs:42`). | wire ✓, durable ✓. Retype the field (wire stays a UUID string via `schema(with="String")`); update ~6 producers; delete the `1493` round-trip branch. |
| **S3** | `meerkat-llm-core/src/provider_runtime/registry.rs:208` | `auth_binding.realm.as_str() != realm.realm_id` — typed `RealmId` downgraded to `&str` to compare against the bare `String` field `RealmConnectionSet.realm_id`. | low | EXISTS — `RealmId` (`connection.rs:110`). | wire ✓, durable ✓. Retype `RealmConnectionSet.realm_id` to `RealmId` (parse at `from_config`); line 208 becomes `RealmId != RealmId`; removes the redundant re-parse at `connection.rs:644`. |
| **S4** | `meerkat-mcp-server/src/lib.rs:1148` (+ `:1171`) | Tri-state read-timeout policy (Default/Fixed/Infinite) as `no_timeout: bool` + `timeout_ms: Option<u64>`; consumers (`2533`/`2651`) reconstruct `Option<Duration>` by precedence; contradictory state silently resolved. | low | EXISTS — `ToolDispatchTimeoutPolicy` (`ops.rs:182`) or a thin wire sibling `StreamReadTimeout`. | wire ✓. Replace the bool+option pair with one tagged-serde field; delete the precedence reconstruction. |
| **S5** | `meerkat-rest/src/auth_endpoints.rs:764` | `body.provider` (String) compared against `auth_profile.provider.as_str()` — typed `Provider` round-tripped through text for a 400 decision; the sibling `complete_login` (`:1142`) already does it typed. | low | EXISTS — `Provider::parse_strict` (`provider.rs:31`). | none. Parse once, compare typed values, keep the string only for the error message. (The adjacent `auth_method` compare at `:778` is *not* a violation — `AuthProfile.auth_method` is itself String with no typed owner; that is N1's concern, not this one.) |
| **S6** | `meerkat/src/factory.rs:1047` | `(provider == "gemini").then(|| executors.get("google"))` — hardcoded gemini=google alias as a string fallback, duplicating `ImageGenerationProviderProfile::provider_aliases()`/`matches_provider_id()`. Provably dead (map never keyed by `"google"`). | low | EXISTS — `image_generation.rs:463-471`. | none. Delete the dead fallback (plain `executors.get(provider)` suffices), or key the map by typed `Provider`. |
| **S7** | `meerkat-cli/src/main.rs:4534` / `:4765` / `:4774` | A connected CLI config-seed/base-url-heal cluster: `LoginProvider::backend_kind()` restates literals the matrix enums own (4534); `ensure_cli_interactive_oauth_config` gates healing on raw-String `==` of `provider`/`backend_kind` (4765); staleness predicate compares against the magic legacy URL literal `"https://chatgpt.com/backend-api"` (4774, duplicated in `meerkat-openai/runtime/mod.rs:88`). | low–medium | EXISTS — `Provider`, `OpenAiBackendKind`/`GoogleBackendKind`; NEW `OpenAiBackendKind::is_legacy_chatgpt_base_url`. | durable ✓ (heals persisted realm config). |

> **S7 is itself a mini-cluster** (3 connected CLI sites + 1 provider-internal twin). I kept it as a labeled singleton because all three sites are confined to one CLI config-seed code path and one repair PR; treating it as a peer of the cross-crate clusters would overstate its blast radius. Repair: route `LoginProvider` through the typed matrix enums (`backend_base_url()` already does), parse config strings via `Provider::parse_strict`/matrix `parse()` at the `4765` gate, and add a typed `is_legacy_chatgpt_base_url` legacy-set owner consulted by both the CLI and the provider-internal `runtime/mod.rs:88`.

---

## Contested-site verdicts (the 4 re-judgments)

| Site | Verdict | Reasoning |
|------|---------|-----------|
| `meerkat-openai/src/live.rs:2974` (`map_openai_live_error` timeout substring) | **UPHOLD REFUTATION (not a violation)** | Verified: the `WebSocket` arm is *unconditionally* `NetworkTimeout`; the `.contains("timed out")` substring only toggles the cosmetic `duration_ms` field. The recoverable/fatal fork keys on the typed `LlmRetryFailureKind::NetworkTimeout` *variant* (`error.rs:446`, machine `meerkat_machine.rs:4788`), never on `duration_ms`, and no backoff computation reads it. The untyped carrier drives **no semantic decision** — it feeds inert telemetry. The stronger refutation (no decision) supersedes the original "provider-internal parse" one. *Code-quality nit only:* `tungstenite::Error::Io` exposes `ErrorKind::TimedOut` and should be matched typed — not a Rule-4 obligation. |
| `meerkat-core/src/connection.rs:571` (`Option<&AuthBindingRef>` is_some) — **4 concurring re-judgments** | **UPHOLD REFUTATION (not a violation)** | Verified the carrier and surrounding resolver. `Some` = explicit fully-typed strict target (`RealmId`/`BindingId`/`Option<ProfileId>`, with `parse`/`Display` deliberately deleted in Wave-b so no opaque `realm:binding` join ferries through); `None` = run the documented best-available candidate ordering. Absence has **exactly one** harmless meaning ("no explicit binding → default policy"); no parallel bool collapses a third state (the orthogonal `allow_env_default` is a separate env-fallback axis). This is the textbook legitimate explicit-or-default `Option<T>` carve-out — `.is_some()` is a pure presence check on an already-canonical typed value, not identity recovery from text. Distinct from cluster M6 (which is the `Option<String>` realm-carrier family). **Note:** while `571` is clean, the *same function family* houses the genuinely-violating `is_env_default()`/`default_<provider>` string oracles (N3/N4) — the contested line is fine; its neighbors are not. |
| `meerkat-mcp-server/src/lib.rs:1767` (prefix-guarded family routers in `tools/call` dispatch) | **UPHOLD REFUTATION (not a violation)** | Verified the dispatcher shape. The `name.starts_with("meerkat_schedule_")` / `"workgraph_"` prefixes select a handler *family*, then forward the full wire `tool_name` to the owning crate which re-dispatches by exact-string match and fails CLOSED (`meerkat-schedule/tools.rs:223` → `other => Err(invalid_arguments)` at `:293`); args parse into typed inputs at the boundary; the outer dispatcher also fails closed (`_ => method_not_found` at `1824`). This is the **named legitimate exemption**: transport/wire method dispatch by name that fails closed at the surface. The tool name *is* the wire identity being dispatched, not parsed for concealed meaning. |

No overturns. All four refutations stand.

---

## Partitions that looked thin / under-swept

1. **The `connection.rs` binding-resolution partition was over-fragmented in the input.** N3 (`is_env_default` slug), N4 (`default_<provider>` name), the contested `571`, and S3 (`registry.rs:208` realm `as_str()`) are all facets of one resolver's stringly-typed identity handling. The sweep produced four+ separate records (some duplicative — `connection.rs:137` and `connection.rs:446` are the *same* `is_env_default` carrier with two consumers). Recommend a **single connection-resolver dogma pass** rather than piecemeal edits; the per-provider-default gap (N4) is the one most likely to have *more* unflagged siblings (any other consumer of `from_inline_api_keys`-minted ids).

2. **The mob-mcp `action: String` item was double-counted** (`lib.rs:3500` low + `lib.rs:3137` medium) — one carrier, folded into N6 at the higher severity. This suggests the panel was running file-level rather than carrier-level dedup; other files with multiple recorded sites (auth-core, CLI main.rs) should be re-checked for the same double-counting before estimating remediation volume.

3. **Two members rest on 2/3 refuter consensus** (S1 surface_metadata reserved-key gate; the mob-result-DTO member of N7) — weaker than the 3/3 majority. Both are real but worth a confirming pass; S1 in particular has a *third* sibling gate (`is_session_authority_metadata_key`) the sweep noted but did not separately flag, hinting the reserved-key-space partition was not fully swept across `meerkat-core`.

4. **Display-text vs decision boundary** was the dominant discriminator that separated confirmed violations from upheld refutations (live.rs:2974, the CLI `status_text()` *was* a violation because it drives a branch; the metadata/log uses of `provider()` were correctly excluded). Any future sweep of `*.as_str()`/`*_text()` call sites should pre-filter on "does a branch read this" — several legitimate display sites in these same files were correctly **not** flagged, indicating that lens was applied consistently.

5. **No partition appears systemically missed**, but N1 (the auth_method oracle) is the one cluster whose typed owner (`NormalizedAuthMethod -> PersistedAuthMode`) is *entirely absent* and whose string table is `pub` and consumed across 6 sites in 4 crates — it is the highest-leverage fix and the most likely to have additional unswept consumers (any code reading `auth_profile().auth_method` to make a decision).