# B: Getting started, deployment, configuration and architecture

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## B-001: Quickstart says roster context is empty although both SDKs now receive the compiled definition

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/quickstart.mdx:189-194`**

```text
The roster callback's `context` argument is
empty on both SDKs (the gateway sends the definition, the SDK dispatchers do
not pass it through)
```

Hosts are instructed to duplicate parsing/configuration state and to avoid a working public callback contract.

**`meerkat-mobkit/src/identity_first/gateway_bridges.rs:392-400`**

```text
let params = json!({ "context": context });
```

GatewayRosterProvider serializes and nests the actual Rust RosterContext in the same envelope the SDK readers consume.

**`sdk/python/meerkat_mobkit/agent_builder.py:753-758`**

```text
context = RosterContext.from_dict(params.get("context") or {})
            specs = await provider.roster(context)
```

Python explicitly passes the decoded context to the roster provider rather than discarding it.

**`sdk/typescript/src/agent-builder.ts:517-525`**

```text
const context = parseRosterContext(params.context ?? {});
      const specs = await this._rosterProvider.roster(context);
```

TypeScript does the same.

**`sdk/python/meerkat_mobkit/identity_first_models.py:653-687`**

```text
mob_definition: dict[str, Any] | None = None
    previous_identities: list[str] = field(default_factory=list)
```

The public callback model carries both useful fields; profile-derived rosters need not reparse a second copy of MOB_TOML.

### Independent adjudication

The present-tense assertion in docs/quickstart.mdx:189-194 is false for the current bundled gateway and SDKs. I followed the actual callback envelope and examined both SDK regression fixtures, then executed the real Python dispatcher directly. It preserves both fields. This is not a promise that every custom or old gateway supplies a definition: empty legacy envelopes still produce optional/default fields.

**`meerkat-mobkit/src/identity_first/gateway_bridges.rs:392-400`**

```text
let context = serde_json::to_value(context)
            .map_err(|e| RosterError::Io(format!("serialize roster context: {e}")))?;
        let params = json!({ "context": context });
```

The implementation serializes the supplied typed context under the SDK-consumed context key; it does not send an empty callback object.

**`sdk/python/tests/test_identity_first_builder_dispatcher.py:527-581`**

```text
assert context.mob_definition["id"] == "team"
        assert context.mob_definition["profiles"]["assistant"]["model"] == "gpt-5.5"
        assert context.previous_identities == ["a:main", "b:main"]
```

The fixture checks received values, not merely model existence. A separate no-file-write asyncio exercise of the actual CallbackDispatcher passed for a populated envelope, context={}, and an absent context.

**`sdk/typescript/tests/identity-first.test.ts:1765-1803`**

```text
assert.deepEqual(seen[0], {
      mobDefinition: {
        id: "team",
        profiles: { assistant: { model: "gpt-5.5" } },
      },
      previousIdentities: ["a:main", "b:main"],
    });
```

The TypeScript regression pins the camelCase callback fields, including the compiled definition; its following assertions cover empty old-gateway contexts.

**Required correction:** Replace the empty-context/duplicate-MOB_TOML-parsing claim with the current typed callback contract: Python context.mob_definition and context.previous_identities; TypeScript context.mobDefinition and context.previousIdentities. The stock gateway supplies its compiled definition; custom/older hosts may omit it, yielding None/null and default empty identities. Retain the profiles-versus-members explanation.

### Changes and final verification

**Changed:** `docs/quickstart.mdx`.

Replaced the empty SDK context and duplicate TOML parsing claims with the typed Python and TypeScript RosterContext fields, the stock compiled-definition input, and optional/default fields for custom or older hosts. Preserved profiles-versus-members semantics and integrated K-005's call-site qualification.

**Validation:** Executed the actual Python CallbackDispatcher against populated, empty-context, and absent-context envelopes: preserved definition and previous identities, with None/[] defaults in both legacy cases. Inspected sdk/typescript/src/agent-builder.ts:517-525 and the adjudicated TypeScript regression. Quickstart Python/TOML syntax and local links pass.

**Final review: pass.** The quickstart now describes both typed SDK contexts, the compiled definition, optional older/custom-host inputs and empty-list defaults. It preserves the profiles-versus-members distinction and incorporates K-005 rather than turning prior identities into a universal membership snapshot. Executed the actual Python callback dispatcher and TypeScript context parser for populated and empty envelopes.

**`docs/quickstart.mdx:199-205`**

```text
Both SDKs pass a typed `RosterContext` to the
roster callback: Python exposes `context.mob_definition` and
`context.previous_identities`; TypeScript exposes `context.mobDefinition` and
`context.previousIdentities`.
```

The obsolete claim that both SDKs discard context is gone.

**`meerkat-mobkit/src/identity_first/gateway_bridges.rs:392-398`**

```text
let params = json!({ "context": context });
```

The live bridge emits the nested context envelope.

**`sdk/python/meerkat_mobkit/agent_builder.py:753-758`**

```text
context = RosterContext.from_dict(params.get("context") or {})
```

The real dispatcher decodes and forwards the context, as independently exercised.

**`sdk/typescript/src/types.ts:2663-2675`**

```text
return { mobDefinition: null, previousIdentities: asStringArray(d.previous_identities) };
```

Missing or null definitions retain the documented TypeScript defaults.

## B-002: Quickstart's console-opening step omits the auth configuration required by its SDK examples

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/quickstart.mdx:126-140`**

```text
Once the runtime is serving HTTP, open the admin console at:
```

A user following the getting-started flow reaches a fail-closed console instead of the promised workbench, with no local explanation of the missing setup.

**`docs/quickstart.mdx:82-106`**

```text
async with await MobKit.builder().gateway("/path/to/rpc_gateway").build() as rt:
```

Neither this Python starter nor the neighboring TypeScript starter configures auth or explicitly opts out.

**`meerkat-mobkit/src/decisions.rs:135-144`**

```text
require_app_auth: true,
```

ConsolePolicy defaults to requiring authentication.

**`meerkat-mobkit/src/runtime.rs:445-451`**

```text
const LOCAL_CONSOLE_JWKS_JSON: &str = r#"{"keys":[]}"#;
```

The stock SDK gateway's fallback decision state trusts no key; leaving auth unset does not provide an anonymous working console.

**`sdk/python/meerkat_mobkit/builder.py:596-605`**

```text
The gateway
        default is fail-closed: without ``.auth(...)`` the console trusts no
        signing key and refuses every request with 401.
```

The existing public opt-out is console_auth_required(False), with the TypeScript consoleAuthRequired(false) equivalent.

### Independent adjudication

The quickstart's Open the console section follows SDK examples that do not select either authenticated operation or the explicit local opt-out. The SDK gateway's default protected console APIs cannot admit anyone. The conditional phrase 'Once the runtime is serving HTTP' does not supply the missing auth prerequisite. This finding is limited to SDK rpc_gateway operation, not every possible Rust host or the standalone gateway, and does not assert that merely downloading the static frontend is denied.

**`docs/quickstart.mdx:81-107`**

```text
async with await MobKit.builder().gateway("/path/to/rpc_gateway").build() as rt:
```

Neither this Python launch nor the neighboring TypeScript builder chain configures console auth or its opt-out before the page suggests using the console.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:3059-3068`**

```text
fn no_auth_config_fallback_is_closed_to_every_caller() {
        let state = minimal_decision_state();

        assert!(state.console.require_app_auth);
        assert_eq!(
            ConsoleAuthPosture::of(&state),
            ConsoleAuthPosture::ClosedToEveryCaller
```

The regression specifically pins the gateway fallback, so this is not an inference from a generic library default alone.

**`meerkat-mobkit/src/runtime.rs:445-449,475-480`**

```text
const LOCAL_CONSOLE_JWKS_JSON: &str = r#"{"keys":[]}"#;
```

The fallback has no verifying key. The adjacent constructor contract distinguishes require_app_auth=true from the explicitly open local-console policy.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:2927-2940`**

```text
"console_require_app_auth": false
```

The opt-out regression verifies that this option actually produces ConsoleAuthPosture::Open.

**Required correction:** Add an SDK-specific prerequisite to Open the console: configure auth and the matching token/proxy handling, or explicitly opt out with .console_auth_required(False) / .consoleAuthRequired(false) only for a loopback-only or authenticated-proxy-protected demo. Do not globally disable authentication or imply that this SDK default applies to every library host.

### Changes and final verification

**Changed:** `docs/quickstart.mdx`.

Added the SDK gateway's fail-closed console prerequisite, configured auth/token/proxy alternative, and explicit Python/TypeScript open-console setters restricted to loopback-only or authenticated-proxy-protected demos. Explained keeping the runtime alive while browsing without removing existing shutdown cleanup.

**Validation:** The real Python builder accepts console_auth_required(False); TypeScript consoleAuthRequired(required: boolean) is present. Verified the no-key default against rpc_gateway's adjudicated fallback and runtime.rs, with wording limited to protected console APIs rather than static frontend downloads. Authentication link resolves.

**Final review: pass.** The added console prerequisite is correctly limited to SDK-launched rpc_gateway protected APIs. It offers configured authentication or a deliberately bounded loopback/proxy opt-out, without claiming static assets require auth or removing shutdown cleanup. The Python setter was executed and the TypeScript setter inspected.

**`docs/quickstart.mdx:138-146`**

```text
For an SDK-launched `rpc_gateway`, protected console APIs are **auth-closed by
default**
```

The previously missing default and setup prerequisite are explicit.

**`docs/quickstart.mdx:142-146`**

```text
loopback-only or authenticated-proxy-protected demo, explicitly add
`.console_auth_required(False)` in Python or `.consoleAuthRequired(false)` in
TypeScript before `.build()`.
```

The opt-out is limited to the intended demo posture.

**`meerkat-mobkit/src/runtime.rs:445-448`**

```text
const LOCAL_CONSOLE_JWKS_JSON: &str = r#"{"keys":[]}"#;
```

The fallback really trusts no signing keys.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:3059-3067`**

```text
ConsoleAuthPosture::ClosedToEveryCaller
```

The existing regression pins the stock no-auth fallback; this test was read, not run.

## B-003: Deployment and configuration falsely describe mobkit_gateway as lacking authentication ingress

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/deployment.mdx:16-18`**

```text
`mobkit_gateway` serves that console **open**: it has
no `auth_config` ingress and builds its decision state with
`require_app_auth = false`, so the loopback bind is its only access boundary.
```

Operators are told built-in authentication is impossible on the console gateway, and the configuration reference misstates the protection of authenticated launches.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:90-94`**

```text
auth_config: Option<Value>,
```

The typed top-level InitParams now has an auth ingress.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:2141-2148`**

```text
if let Some(auth) = params.auth_config.as_ref() {
        let configured = meerkat_mobkit::console_auth_config::parse_console_auth_config(auth)
            .map_err(|error| anyhow!("{error}"))?;
        decisions.auth = configured.auth;
        decisions.trusted_oidc = configured.trusted_oidc;
        decisions.console.require_app_auth = true;
```

Before constructing the served router, the gateway installs configured auth and requires it.

**`meerkat-mobkit/src/console_auth_config.rs:11-28`**

```text
if provider == "oidc" {
        return parse_oidc_auth_config(object);
    }
    if provider != "jwt" {
```

The shared ingress accepts both JWT shared-secret and OIDC configurations.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:1534-1541`**

```text
&runtime_decision_state(ConsoleUiConfig::default(), console_read_only),
```

Important limit on the correction: the earlier non-loopback admission check still receives the open default state, not the later configured state. This audit is not asking for a runtime change; allow_remote remains necessary on mobkit_gateway.

**`docs/reference/configuration.mdx:845-850`**

```text
Its console is
always open,
```

The same stale claim is repeated here and in the Console auth by surface table at line 874; deployment repeats 'no auth ingress' at line 145.

### Independent adjudication

Both documents' 'no auth ingress' and 'always open' statements are contradicted by the typed init input and the actual decision state passed to the HTTP router. I also traced the earlier exposure check: it uses a separately constructed open state, so the proposed documentation correction must not incorrectly advertise top-level auth_config as sufficient to admit a non-loopback standalone bind.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:90-94,2141-2148,2173-2178`**

```text
auth_config: Option<Value>,
...
if let Some(auth) = params.auth_config.as_ref() {
        let configured = meerkat_mobkit::console_auth_config::parse_console_auth_config(auth)
            .map_err(|error| anyhow!("{error}"))?;
        decisions.auth = configured.auth;
        decisions.trusted_oidc = configured.trusted_oidc;
        decisions.console.require_app_auth = true;
```

Top-level auth_config is read and its resulting protected decision state is used by build_reference_app_router, rather than being a dead field.

**`meerkat-mobkit/src/console_auth_config.rs:11-28,90-96`**

```text
if provider == "oidc" {
        return parse_oidc_auth_config(object);
    }
    if provider != "jwt" {
```

The shared parser accepts JWT and OIDC; the correction should refer to this existing shape rather than inventing a standalone-only format.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:781-792,1533-1542`**

```text
require_app_auth: false,
...
&runtime_decision_state(ConsoleUiConfig::default(), console_read_only),
```

The pre-bootstrap admission check receives the open default, not the later auth overlay. Therefore allow_remote remains necessary for a non-loopback mobkit_gateway bind.

**Required correction:** Update deployment's introduction and exposure-gate paragraph and configuration's standalone-policy paragraph and auth-surface table: mobkit_gateway is open by default when top-level auth_config is absent, but accepts the shared JWT/OIDC auth configuration and then requires app auth on its protected served surfaces. Contrast this top-level input with rpc_gateway runtime_options.auth_config. Explicitly retain the current standalone non-loopback allow_remote requirement because its earlier exposure gate does not consume the later auth overlay. No runtime change is requested.

### Changes and final verification

**Changed:** `docs/guides/deployment.mdx`, `docs/reference/configuration.mdx`.

Corrected all owned 'no auth ingress'/'always open' claims: standalone mobkit_gateway is open only when top-level auth_config is absent and accepts the shared JWT/OIDC shape. Distinguished rpc_gateway's runtime_options.auth_config. Retained standalone non-loopback allow_remote admission because its gate precedes the auth overlay, and qualified the adjacent exposure log as admission-time posture.

**Validation:** Read console_auth_config.rs:11-28,90-160, mobkit_gateway.rs:1515-1548 and 2132-2155. The gate uses the open default before the HTTP router's auth overlay sets require_app_auth=true. Source-contract assertions and owned-document link checks pass. No gateway behavior changed.

**Changed:** `docs/guides/deployment.mdx`.

Replaced the admission-time WARN claim with the final serving auth posture, including standalone auth_config overlays. Located the warning at serving startup while preserving the distinct early open bind-gate state and the standalone non-loopback allow_remote requirement.

**Validation:** Independently read meerkat-mobkit/src/bin/mobkit_gateway.rs:1529-1542,2141-2148,2171-2178 and src/gateway_composition.rs:631-652. The exposure gate runs before the auth overlay; the warning runs after it and receives the same final decisions passed to the router. Also checked rpc_gateway.rs:13287-13296. Source-order/document assertions and MDX compilation passed.

**Final review: pass.** Independently re-reviewed the residual correction. Deployment now separates the early open-state exposure gate from the later final-serving-state WARN. The standalone non-loopback allow_remote requirement is preserved, while the warning correctly includes auth_config overlays. Re-read both gateway warning call sites and the shared classifier. B-R001 is resolved.

**`docs/guides/deployment.mdx:163-167`**

```text
stderr naming the bound address and the final serving auth posture. On
`mobkit_gateway` this includes any configured `auth_config` overlay, unlike the
open decision state used by its earlier exposure gate.
```

The corrected warning description now matches the call order and preserves the earlier-gate distinction.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:1534-1542`**

```text
&runtime_decision_state(ConsoleUiConfig::default(), console_read_only),
```

The early gate does use the open default; retain that part of the correction.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:2141-2148`**

```text
decisions.console.require_app_auth = true;
```

The auth overlay is installed before warning or router construction.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:2171-2177`**

```text
warn_on_non_loopback_bind(
        meerkat_mobkit::gateway_composition::GatewaySurface::MobkitGateway,
        http_binding.local_addr(),
        &decisions,
    );
```

The only warning call receives the overlaid decision state, which also feeds the router.

**`meerkat-mobkit/src/gateway_composition.rs:639-644`**

```text
ConsoleAuthPosture::Enforced => "console app auth is enforced",
```

Configured authentication changes the actual WARN text.

## B-004: IPv6 wildcard binds are documented as returning an IPv4 loopback URL

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/deployment.mdx:112`**

```text
the bound port at `127.0.0.1` for `0.0.0.0` / `::` binds
```

A proxy or client configured from the documented rule can dial the wrong address family, especially on IPv6-only listeners.

**`meerkat-mobkit/src/gateway_composition.rs:454-464`**

```text
IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bound.port())
        }
```

The helper preserves address family: :: becomes ::1, not 127.0.0.1.

**`meerkat-mobkit/src/gateway_composition.rs:416-426`**

```text
format!("http://{}", self.reachable_addr())
```

SocketAddr formatting produces http://[::1]:PORT for the IPv6 case.

**`docs/reference/configuration.mdx:107`**

```text
`127.0.0.1` for `0.0.0.0` and `::` binds
```

The reference repeats the same incorrect returned-URL contract.

### Independent adjudication

Both documented wildcard-to-127.0.0.1 rules are wrong specifically for IPv6. I checked the helper used by http_base_url and its two-family regression test, not only an explanatory comment. Concrete addresses stay unchanged.

**`meerkat-mobkit/src/gateway_composition.rs:414-426,454-464`**

```text
IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bound.port())
        }
```

http_base_url formats reachable_addr, which calls this mapping; the IPv6 result is bracketed ::1, not an IPv4 endpoint.

**`meerkat-mobkit/src/gateway_composition.rs:1950-1966`**

```text
loopback_reachable_addr("[::]:8080".parse()?),
            "[::1]:8080".parse()?
```

The regression explicitly verifies IPv6 and separately verifies IPv4 wildcard and concrete-address behavior.

**Required correction:** In deployment's base-URL table and configuration's http_listen row, state that wildcard binds map to loopback of the same family: 0.0.0.0:PORT produces http://127.0.0.1:PORT and [::]:PORT produces http://[::1]:PORT. Concrete bound IP addresses and the bound port are preserved.

### Changes and final verification

**Changed:** `docs/guides/deployment.mdx`, `docs/reference/configuration.mdx`.

Both HTTP base-URL descriptions now map IPv4 and IPv6 wildcard binds to loopback of the same family, preserving the bound port and concrete addresses.

**Validation:** Verified gateway_composition.rs's reachable-address helper and existing IPv4/IPv6 regression contract; source assertion confirms Ipv6Addr::LOCALHOST. Both owned descriptions show http://127.0.0.1:PORT versus http://[::1]:PORT. Rust SDK occurrence is a cross-scope request, not edited here.

**Changed:** `docs/sdks/rust.mdx`.

Corrected the repeated IPv4-only URL claim in the owned Rust SDK HTTP section: wildcard IPv4 becomes http://127.0.0.1:PORT, wildcard IPv6 becomes http://[::1]:PORT, and concrete addresses and the bound port are preserved.

**Validation:** Read GatewayHttpBinding::http_base_url and loopback_reachable_addr in gateway_composition.rs:414-464. Verified both family-specific mappings are present in the corrected Rust SDK prose.

**Final review: pass.** Complete across all three known occurrences: deployment, configuration, and the D-owned Rust SDK now preserve the address family and bound port, mapping IPv4 wildcard to 127.0.0.1 and IPv6 wildcard to bracketed ::1. Concrete addresses remain unchanged.

**`docs/guides/deployment.mdx:114-114`**

```text
`[::]:PORT` becomes `http://[::1]:PORT`
```

The deployment base-URL table includes the IPv6 case.

**`docs/reference/configuration.mdx:112-112`**

```text
`[::]:PORT` becomes `http://[::1]:PORT`
```

The configuration row agrees.

**`docs/sdks/rust.mdx:315-319`**

```text
family: `0.0.0.0:PORT` becomes `http://127.0.0.1:PORT`, and `[::]:PORT`
becomes `http://[::1]:PORT`.
```

The cross-scope Rust SDK repetition is closed.

**`meerkat-mobkit/src/gateway_composition.rs:454-463`**

```text
SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bound.port())
```

The executable helper preserves family and port.

**Final review: pass.** Reviewed the specifically assigned repeated Rust SDK occurrence. It now maps IPv4 and IPv6 wildcards to loopback of the same family, retaining concrete addresses and the actual bound port. Both the helper implementation and existing two-family regression support this wording. This is not a separate sign-off on the scope-B deployment/configuration pages.

**`docs/sdks/rust.mdx:316-320`**

```text
family: `0.0.0.0:PORT` becomes `http://127.0.0.1:PORT`, and `[::]:PORT`
becomes `http://[::1]:PORT`. Concrete bound addresses and the bound port are
preserved.
```

The repeated IPv4-only claim has been fully corrected.

**`meerkat-mobkit/src/gateway_composition.rs:454-464`**

```text
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bound.port())
        }
        _ => bound,
```

The actual mapping preserves address family and concrete addresses; http_base_url formats this SocketAddr.

**`meerkat-mobkit/src/gateway_composition.rs:1950-1966`**

```text
            loopback_reachable_addr("[::]:8080".parse()?),
            "[::1]:8080".parse()?
```

The regression independently encodes the IPv6 case. Its source was inspected, not executed.

## B-005: Schedule watchdog diagnostics are presented as unconditional on every gateway launch

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/guides/deployment.mdx:163`**

```text
Both gateways probe the shared schedule service at boot and every 60 seconds.
```

Operators can interpret the absence of watchdog boot/heartbeat records on an ephemeral or degraded launch as a logging or scheduler failure rather than an unconfigured service.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:1872-1875`**

```text
// Ephemeral sessions have no persistent service; the runtime-backed
        // schedule firing host (and thus schedule tools) is persistent-only.
        (spec, None, workgraph_service, None)
```

The default ephemeral standalone launch supplies no schedule host inputs.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:13187-13195`**

```text
schedule host (ephemeral mode, or schedule store unavailable):
```

The SDK gateway also has a no-host branch, which returns no schedule host and no watchdog.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:13187-13196`**

```text
(None, None)
```

This is the actual fallback result for absent schedule_host_inputs, not an always-on watchdog.

### Independent adjudication

The unconditional boot/every-60-seconds statement is false when the gateway has no schedule_host_inputs, including ephemeral launches. The original proposed correction is slightly too broad: the resident watchdog starts when the attached persistent schedule inputs are composed even if the firing host subsequently fails to spawn; only the immediate boot probe is conditional on successful host startup. I narrowed the correction to preserve that distinction.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:1872-1875,2081-2089,2117-2121`**

```text
(spec, None, workgraph_service, None)
...
} else {
        (None, None)
    };
```

The ephemeral bootstrap provides no schedule inputs, and the later no-input branch installs neither the schedule host nor the watchdog.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:13186-13196`**

```text
schedule host (ephemeral mode, or schedule store unavailable):
```

The SDK gateway's corresponding branch returns (None, None), likewise skipping the watchdog.

**`meerkat-mobkit/src/gateway_composition.rs:1330-1374`**

```text
let watchdog = crate::schedule_wiring::spawn_schedule_claim_watchdog(
        schedule_service.clone(),
        schedule_store_path.clone(),
        watchdog_config,
    );
...
if schedule_host.is_some() {
```

The watchdog is created before the firing-host result; the immediate probe is inside the successful-host branch. Do not describe all watchdog operation as contingent on a live firing host.

**`meerkat-mobkit/src/schedule_wiring.rs:1187-1194`**

```text
poll_interval: Duration::from_mins(1),
            overdue_threshold: Duration::from_mins(2),
            heartbeat_polls: 10,
```

The stated cadence, overdue threshold, and heartbeat count remain valid when the watchdog exists.

**Required correction:** Qualify the section: the gateways install the resident 60-second watchdog when persistent schedule-service inputs are available; ephemeral launches and launches whose schedule store is unavailable do not install it. An immediate boot probe runs after the firing host successfully starts; if host startup fails, the resident watchdog still exists and startup logs the failure. Preserve the verified 120-second overdue threshold and ten-poll heartbeat.

### Changes and final verification

**Changed:** `docs/guides/deployment.mdx`.

Limited watchdog installation to launches with persistent schedule-service inputs. Distinguished the resident watchdog, which survives firing-host startup failure, from the immediate boot probe, which requires successful firing-host startup. Preserved the 60-second cadence, 120-second overdue threshold and ten-poll heartbeat.

**Validation:** Read gateway_composition.rs:1320-1385: watchdog creation precedes host startup and the immediate probe is under if schedule_host.is_some(). Compared gateway no-input branches and schedule_wiring.rs defaults from the adjudication. No live scheduler was launched.

**Final review: pass.** The installation condition, ephemeral/unavailable-store exclusions, immediate-probe success condition and resident-watchdog survival after host-start failure all match the production branches. The verified cadence, overdue threshold and heartbeat remain intact.

**`docs/guides/deployment.mdx:171-180`**

```text
An immediate
boot probe runs after the firing host successfully starts; if host startup
fails, startup logs that failure and the resident watchdog still runs.
```

This preserves the adjudication's important watchdog-versus-host distinction. Citation refreshed after the preceding WARN paragraph changed length.

**`meerkat-mobkit/src/gateway_composition.rs:1338-1355`**

```text
let watchdog = crate::schedule_wiring::spawn_schedule_claim_watchdog(
```

Resident watchdog creation precedes firing-host creation and the conditional immediate probe.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:2118-2121`**

```text
(schedule_host, Some(watchdog))
    } else {
        (None, None)
```

Absent schedule inputs install no watchdog.

**`meerkat-mobkit/src/schedule_wiring.rs:1189-1193`**

```text
poll_interval: Duration::from_mins(1),
            overdue_threshold: Duration::from_mins(2),
            heartbeat_polls: 10,
```

The unchanged documented timing values match the live defaults.

## B-006: ModuleConfig's raw Rust/serde type is assigned defaults it does not have

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:44-45`**

```text
| `args` | `Vec<String>` | `[]` | Command-line arguments |
| `restart_policy` | `RestartPolicy` | `OnFailure` | Process restart behavior |
```

A reader omitting these fields when deserializing the advertised Rust type gets missing-field errors, despite the reference's default column.

**`meerkat-mobkit/src/types.rs:94-100`**

```text
pub struct ModuleConfig {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub restart_policy: RestartPolicy,
}
```

ModuleConfig has neither a Default implementation/derive nor serde default attributes for args or restart_policy; raw construction/deserialization requires both.

**`meerkat-mobkit/src/decisions.rs:91-101`**

```text
#[serde(default)]
    pub args: Vec<String>,
    pub restart_policy: Option<RestartPolicy>,
```

The defaults belong to the separate TrustedModuleDecl manifest reader.

**`meerkat-mobkit/src/decisions.rs:219-224`**

```text
restart_policy: module.restart_policy.unwrap_or(RestartPolicy::OnFailure),
```

The manifest loader explicitly materializes the OnFailure default when constructing ModuleConfig.

### Independent adjudication

The table is explicitly headed ModuleConfig, not the trust-manifest adapter. Its []/OnFailure defaults are therefore misleading for raw Rust construction and serde input. The values do exist elsewhere, so removing them from all configuration documentation would be an overcorrection.

**`meerkat-mobkit/src/types.rs:92-100`**

```text
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleConfig {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub restart_policy: RestartPolicy,
}
```

Neither the struct nor either field has Default/serde(default); the fields are required at this boundary.

**`meerkat-mobkit/src/decisions.rs:90-99,215-224`**

```text
#[serde(default)]
    pub args: Vec<String>,
    pub restart_policy: Option<RestartPolicy>,
...
restart_policy: module.restart_policy.unwrap_or(RestartPolicy::OnFailure),
```

The separate manifest model supplies the omitted-args default and its conversion supplies OnFailure.

**Required correction:** Mark args and restart_policy as required/no raw default in the ModuleConfig table. Explain that the trust-manifest loader supplies [] and OnFailure when omitted, rather than attributing those defaults to ModuleConfig itself. Retain the accurate manifest-default documentation.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Marked ModuleConfig args and restart_policy required at the raw Rust/serde boundary, and located []/OnFailure defaults in the trust-manifest adapter instead. Preserved the existing manifest defaults.

**Validation:** Compared types.rs:92-100 and decisions.rs:90-99,215-224 from the independently checked contract. Added trust-manifest heading link passes local-anchor validation; the manifest TOML example parses.

**Final review: pass.** The raw ModuleConfig inventory now requires every field and accurately locates omitted-args/restart defaults in the trust-manifest adapter. The separate manifest defaults remain correct, and its new anchor resolves.

**`docs/reference/configuration.mdx:44-50`**

```text
Raw Rust construction and serde deserialization of `ModuleConfig` require all
four fields.
```

The incorrect raw-type defaults are removed.

**`meerkat-mobkit/src/types.rs:94-100`**

```text
pub args: Vec<String>,
    pub restart_policy: RestartPolicy,
```

Neither raw field is defaulted.

**`meerkat-mobkit/src/decisions.rs:90-95`**

```text
#[serde(default)]
    pub args: Vec<String>,
    pub restart_policy: Option<RestartPolicy>,
```

Only the manifest model makes omission possible.

**`meerkat-mobkit/src/decisions.rs:215-219`**

```text
restart_policy: module.restart_policy.unwrap_or(RestartPolicy::OnFailure),
```

The adapter materializes the stated restart default.

## B-007: Configured gating risk is described as a missing-value default rather than an override

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:101`**

```text
supplies a configured `risk_tier` for `mobkit/gating/evaluate` requests that omit `risk_tier`.
```

Callers are led to expect that sending risk_tier themselves bypasses the configured value, and cannot explain why their supplied tier is replaced.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6687-6707`**

```text
params.insert("risk_tier".to_string(), Value::String(risk_tier.clone()));
```

For a matched configured action the gateway unconditionally replaces the request tier, including a caller-supplied value.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:841-857`**

```text
fn configured_risk_tier_overrides_a_caller_supplied_tier() {
```

The targeted regression fixture supplies r0 for an action configured r3 and checks that the rewritten request contains r3.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6694-6705`**

```text
"gating risk_tier supplied by caller overridden by configured policy"
```

A disagreement is logged rather than silently treating the configured table as merely advisory.

### Independent adjudication

For the SDK gateway request path that applies the loaded policy, a configured tier is authoritative even when the caller supplies another tier. The missing-only wording is false. I traced the helper's live call site too: this evidence proves the stdio SDK request boundary and must not be widened into a claim that every HTTP/library gating entrypoint runs this rewriting helper.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6680-6707`**

```text
params.insert("risk_tier".to_string(), Value::String(risk_tier.clone()));
```

Once the action matches, insertion is unconditional. A conflicting caller string produces a warning before replacement.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:841-857,860-886`**

```text
fn configured_risk_tier_overrides_a_caller_supplied_tier()
```

The fixture asserts r0 becomes configured r3, an omitted tier gets r3, and an action absent from the table keeps its supplied tier.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:13648-13661`**

```text
let request_line =
                apply_gateway_runtime_config_to_request(&request_line, &gateway_options.gating);
```

This is the actual stdin dispatch loop, not a test-only function or unused policy table.

**Required correction:** Describe gating_config_path as loading action tiers that override caller-supplied risk_tier for matching mobkit/gating/evaluate requests through rpc_gateway's SDK/stdio dispatcher; conflicting supplied string tiers are warned about. Unconfigured actions retain ordinary request behavior. Remove the missing-value-only limitation without promising equivalent rewriting on unrelated HTTP/library entrypoints.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Changed gating policy from a missing-only default to an authoritative action-tier override at the SDK/stdio dispatcher. Conflicting supplied string tiers warn; unconfigured actions retain ordinary behavior. Explicitly avoided a universal HTTP/library guarantee.

**Validation:** Source-contract assertion confirms unconditional params.insert for matched action risk_tier in rpc_gateway.rs. The adjudicated live stdio call site and override/missing/unconfigured regression cases support the narrowed wording.

**Changed:** `docs/api/rpc.mdx`, `docs/reference/configuration.mdx`.

Restored the prior conditional caller requirement for risk_tier: a matching rpc_gateway SDK/stdio policy fills an omitted tier and overrides any supplied tier. Updated the RPC introduction and Required column as well as the configuration row. Retained mandatory caller tiers for unconfigured actions, warnings for conflicting supplied string tiers, and the explicit lack of an unrelated HTTP/library rewriting guarantee.

**Validation:** Read meerkat-mobkit/src/bin/rpc_gateway.rs:6671-6712,13659-13660 and src/rpc/gating_methods.rs:91-113. The live stdio pre-dispatch helper unconditionally inserts the configured tier once the action matches, including when the caller omits it; the ordinary downstream parser still requires a string tier. Source/document assertions and both MDX compilations passed.

**Final review: pass.** The action-tier override remains authoritative for matching SDK/stdio requests. The residual wording now explicitly states that a matching policy also fills an omitted caller tier; unconfigured actions still require one. The RPC Required column agrees, the warning remains limited to conflicting string claims, and no universal HTTP/library rewriting guarantee is introduced. Independently traced the rewrite before the ordinary required-field parser.

**`docs/reference/configuration.mdx:106-106`**

```text
the configured `risk_tier` fills an omitted tier or overrides any caller-supplied tier; a conflicting supplied string tier logs a warning.
```

Both filling and overriding are explicit without making caller input unconditionally required.

**`docs/api/rpc.mdx:685-685`**

```text
yes, unless matching gateway policy supplies it
```

The cross-scope API table agrees with the conditional caller requirement.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6694-6707`**

```text
params.insert("risk_tier".to_string(), Value::String(risk_tier.clone()));
```

Insertion is unconditional after matching the configured action.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:13659-13660`**

```text
apply_gateway_runtime_config_to_request(&request_line, &gateway_options.gating);
```

The verified production call site is the stdio dispatch loop.

**`meerkat-mobkit/src/rpc/gating_methods.rs:109-113`**

```text
.ok_or(GatingParamsError::RiskTierRequired)?;
```

Without a matching rewrite, the ordinary parser still requires the caller's string tier.

## B-008: The exhaustive configuration reference omits the stock session cap and automatic delegate-retirement controls

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:7`**

```text
This page documents every configuration surface.
```

Readers configuring larger rosters cannot discover the stock capacity limit from the claimed exhaustive reference, and ephemeral delegate owners are not told that their idle members are automatically retired or how to disable that behavior.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:398-405`**

```text
max_sessions: 16,
```

The SDK gateway has a consequential default session capacity, but the runtime_options table at documentation lines 95-118 has no max_sessions row.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6292-6302`**

```text
"runtime_options.max_sessions must be greater than zero"
```

This is a supported positive-integer init option, not an inaccessible internal detail.

**`meerkat-mobkit/src/runtime.rs:624-642`**

```text
fn default_implicit_delegate_idle_retire_secs() -> Option<u64> {
    Some(300)
}

fn default_implicit_delegate_idle_sweep_interval_ms() -> u64 {
    10_000
}
```

Implicit delegate retirement is enabled by default at 300 seconds, swept every 10,000ms; neither setting appears in the configuration reference.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6323-6349`**

```text
parsed.runtime_options.implicit_delegate_idle_retire_secs = if value.is_null() {
            None
```

The gateway exposes the idle timeout as nonnegative seconds or null (disabled), plus a positive sweep-interval option.

**`meerkat-mobkit/src/unified_runtime/implicit_delegate_retirement.rs:19-34`**

```text
Duration::from_millis(options.implicit_delegate_idle_sweep_interval_ms.max(1_000));
```

These values actually drive the active retirement task, and sweep intervals below one second are clamped.

**`sdk/python/meerkat_mobkit/builder.py:660-674`**

```text
def implicit_delegate_idle_retirement(
        self, seconds: int | None
    ) -> MobKitBuilder:
```

The retirement policy is also a supported public SDK setter, not merely raw-wire implementation trivia.

### Independent adjudication

This is a concrete omission from the existing claimed exhaustive runtime_options inventory, not a newly added prior-commit omission. The options are accepted, have consequential defaults, and are used by real session construction/retirement code. I checked retirement eligibility and overrides to avoid implying every durable identity is reaped. In particular null disables the default timeout, not explicit per-member numeric overrides.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:398-405,6293-6302,6324-6347`**

```text
max_sessions: 16,
...
parsed.runtime_options.implicit_delegate_idle_retire_secs = if value.is_null() {
            None
```

The stock session cap is 16. The wire parser accepts only positive max_sessions, a nonnegative integer/null retirement timeout, and a positive sweep interval.

**`meerkat-mobkit/src/runtime.rs:624-643`**

```text
fn default_implicit_delegate_idle_retire_secs() -> Option<u64> {
    Some(300)
}

fn default_implicit_delegate_idle_sweep_interval_ms() -> u64 {
    10_000
}
```

These defaults are installed by RuntimeOptions::default, used in GatewayRuntimeOptions::default.

**`meerkat-mobkit/src/unified_runtime/implicit_delegate_retirement.rs:19-34,199-240`**

```text
Duration::from_millis(options.implicit_delegate_idle_sweep_interval_ms.max(1_000));
...
is_primary_mob
        && (per_delegate_override.is_some() || labels.contains_key(DELEGATE_IDLE_RETIRE_SECS_LABEL))
```

The real task enforces a one-second minimum sweep. Implicit-mob members are eligible, while primary-mob members require an override/label; explicit per-member Seconds overrides can still apply when the default is None.

**`sdk/typescript/src/builder.ts:703-719`**

```text
implicitDelegateIdleRetirement(seconds: number | null): this {
...
maxSessions(maxSessions: number): this {
```

These public setters are emitted as implicit_delegate_idle_retire_secs and max_sessions by sdk/typescript/src/runtime.ts:816-822.

**`sdk/python/meerkat_mobkit/builder.py:660-674`**

```text
def implicit_delegate_idle_retirement(
        self, seconds: int | None
    ) -> MobKitBuilder:
```

Python exposes the timeout setter, emitted by runtime.py:649-652. A scoped SDK source search found no Python max_sessions setter or either SDK sweep-interval setter.

**Required correction:** Add the three runtime_options rows: max_sessions is a positive session-cap integer, stock default 16, exposed by TypeScript .maxSessions(n) and raw init for Python; implicit_delegate_idle_retire_secs is a nonnegative integer/null, default 300 seconds, with null disabling the runtime default; implicit_delegate_idle_sweep_interval_ms is positive, default 10000ms, effectively clamped to at least 1000ms, raw-init only. Name both SDK timeout setters. Explain that retirement applies to implicit-mob members and explicitly opted-in primary-mob members, honors per-member overrides/disablement, and does not unconditionally retire the durable roster.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Added max_sessions (positive, stock default 16), implicit_delegate_idle_retire_secs (nonnegative/null, default 300), and implicit_delegate_idle_sweep_interval_ms (positive, default 10000, effective minimum 1000). Documented SDK/raw-init availability and an anchored subsection explaining eligible mobs, primary-mob opt-in, numeric/default/disabled overrides, and why null does not cancel explicit numeric timeouts.

**Validation:** Read rpc_gateway.rs:6280-6355 and implicit_delegate_retirement.rs:19-34,199-240; source assertions confirm defaults, clamp and eligibility. Executed Python timeout setters with None and 300; checked TypeScript setters and member-label constants. New subsection and roster links pass.

**Final review: pass.** All three options, accepted types and defaults are present. The SDK availability statements match the setters/serializers. Primary-mob opt-in, implicit-mob eligibility, explicit disabled/numeric/default overrides, label fallback, and null disabling only the runtime default match the reaper. Executed Python null/zero/300 setter probes.

**`docs/reference/configuration.mdx:118-120`**

```text
Session-capacity limit, default `16` in the stock gateway.
```

The formerly omitted session cap is explicit.

**`docs/reference/configuration.mdx:119-119`**

```text
`null` disables the runtime default, not explicit per-member numeric overrides.
```

The important override caveat is retained.

**`docs/reference/configuration.mdx:136-139`**

```text
Idle retirement applies to members of implicit delegation mobs and to
explicitly opted-in primary-mob members, not unconditionally to the durable
identity roster.
```

The correction does not imply durable roster-wide reaping.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:398-405`**

```text
max_sessions: 16,
```

The stock capacity matches the table.

**`meerkat-mobkit/src/runtime.rs:624-630`**

```text
Some(300)
```

The adjacent default functions return 300 seconds and 10000 milliseconds.

**`meerkat-mobkit/src/unified_runtime/implicit_delegate_retirement.rs:23-25`**

```text
Duration::from_millis(options.implicit_delegate_idle_sweep_interval_ms.max(1_000));
```

The effective one-second minimum is correctly described.

**`meerkat-mobkit/src/unified_runtime/implicit_delegate_retirement.rs:199-239`**

```text
Some(DelegateIdleRetireOverride::RuntimeDefault) => return default_idle_after,
```

Read the complete candidate/override/label helpers; their ordering agrees with the added subsection.

## B-010: Storage-option tables exclude the accepted in_memory compatibility spelling

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:113-114`**

```text
the only accepted declaration is `{ "storage": "memory" }`
```

A reference intended to define accepted wire input incorrectly marks existing compatible configurations invalid.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6474-6477`**

```text
"memory" | "in_memory" => Box::new(InMemoryEventLogStore::default()),
```

event_log accepts in_memory in addition to memory and null, contrary to 'any other storage kind fails initialization' in its row.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6572-6579`**

```text
if !matches!(storage, "memory" | "in_memory") {
```

runtime_store also accepts the in_memory spelling; its row's 'only accepted declaration' is too narrow.

### Independent adjudication

The implementation's parser branches accept in_memory for both settings even though adjacent source comments/error wording, like the docs, mention only memory. I treated executable branches as authoritative. The explicit exclusion of every other storage spelling is thus false, not merely an opportunity to list a preferred alias.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6473-6482`**

```text
"memory" | "in_memory" => Box::new(InMemoryEventLogStore::default()),
        "null" => Box::new(meerkat_mobkit::unified_runtime::NullEventLogStore),
```

The operational event-log parser has two synonymous in-memory spellings and a distinct null backend.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6564-6581`**

```text
if !matches!(storage, "memory" | "in_memory") {
```

The runtime-store parser explicitly admits both spellings and returns the same ephemeral declaration.

**Required correction:** In both configuration rows, recommend memory while documenting in_memory as an accepted compatibility alias. Keep null limited to event_log and retain the explicit declared-ephemeral semantics. Do not modify the parsers or add unsupported persistent backend declarations.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Documented memory as preferred and in_memory as an accepted compatibility alias for event_log and runtime_store; kept null limited to the operational event-log storage kind and preserved declared-ephemeral semantics.

**Validation:** Source assertions match both executable parser branches in rpc_gateway.rs:6473-6482 and 6564-6581; read runtime-store parser directly. Did not rely on stale parser comments/error wording that mention only memory.

**Final review: pass.** Both rows now admit in_memory alongside preferred memory. The null backend remains event-log-only, and the correction preserves explicitly ephemeral rather than fallback semantics.

**`docs/reference/configuration.mdx:121-122`**

```text
"in_memory"
```

Both configuration rows carry the compatibility spelling.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6474-6476`**

```text
"memory" | "in_memory" => Box::new(InMemoryEventLogStore::default()),
```

The event-log parser actually admits both.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6564-6578`**

```text
if !matches!(storage, "memory" | "in_memory") {
```

The runtime-store parser admits precisely the two ephemeral spellings.

## B-011: Runtime-store persistence is described as the unconditional gateway default

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:114`**

```text
The store (`runtime.sqlite` — session resume, archive, retire) is persistent SQLite by default
```

SDK users who omit persistent_state, including the basic Python quickstart shape, can incorrectly expect runtime/session state to survive a gateway restart.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11033-11036`**

```text
let persistent_state = params
        .get("persistent_state")
```

Persistent storage is an optional top-level launch declaration, not implied by the SDK's --persistent stdio protocol mode.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12165-12166`**

```text
let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
```

Without persistent_state the runtime store is in-memory even when no runtime_store option was supplied.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11717-11734`**

```text
let runtime_db_path = storage_layout.runtime_db();
```

SQLite/default-fail-closed behavior belongs to the separate persistent_state branch.

### Independent adjudication

The runtime_store row lacks the launch-mode condition on its SQLite default. I traced the top-level persistent_state branch and its else branch; stdio persistent-process operation alone does not select SQLite. This correction concerns the runtime store and must not conflate its durability with every separately composed storage slot.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11033-11036,11581-11594`**

```text
let persistent_state = params
        .get("persistent_state")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from);
...
) = if let Some(ref state_path) = persistent_state {
```

The durable/ephemeral split is selected by the optional top-level state path, not an unconditional gateway default.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11717-11743,12141-12166`**

```text
match meerkat_runtime::store::SqliteRuntimeStore::new(&runtime_db_path) {
...
let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
```

The persistent branch defaults to SQLite unless explicitly declared ephemeral; the no-persistent_state branch creates InMemoryRuntimeStore.

**`sdk/python/meerkat_mobkit/builder.py:43`**

```text
persistent_state: str | None = None
```

The Python builder does not turn an unconfigured quickstart into a durable launch by supplying a default persistent_state path.

**Required correction:** Qualify the runtime_store row: when top-level persistent_state is supplied, runtime storage defaults to runtime.sqlite and SQLite-open failure fails initialization unless an explicit memory/in_memory declaration selected ephemeral runtime storage. Without persistent_state the gateway uses an in-memory runtime store. Explicitly distinguish --persistent stdio/process mode from durable storage configuration.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Made runtime.sqlite's default conditional on top-level persistent_state, retained fail-closed SQLite opening and explicit memory/in_memory opt-out, and documented the in-memory no-persistent_state branch. Explicitly separated --persistent stdio lifetime from durable storage configuration.

**Validation:** Checked the adjudicated persistent_state branch at rpc_gateway.rs:11581-11594,11717-11743 and no-state branch at 12141-12166; source assertion confirms InMemoryRuntimeStore::new(). The row limits its durability promise to runtime-store state, not every independently composed storage slot.

**Final review: pass.** The runtime-store SQLite default is conditional on top-level persistent_state; the explicit ephemeral override and no-state in-memory branch are both described. The new paragraph separates long-lived stdio mode from durable storage without promising durability for unrelated slots.

**`docs/reference/configuration.mdx:122-122`**

```text
Without `persistent_state`, the gateway uses `InMemoryRuntimeStore` even when this option is omitted.
```

The previously unconditional persistence promise is corrected.

**`docs/reference/configuration.mdx:130-132`**

```text
The gateway's `--persistent` flag selects a long-lived stdio/process protocol;
it does not itself configure durable storage.
```

The two meanings of persistence are explicitly separated.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11581-11594`**

```text
) = if let Some(ref state_path) = persistent_state {
```

The launch's state path selects the durable branch.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11727-11742`**

```text
) = if gateway_options.runtime_store_ephemeral {
```

Even a persistent launch can explicitly select ephemeral runtime storage.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12165-12166`**

```text
Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
```

The no-persistent_state branch is genuinely in memory.

## B-012: Retired selector compatibility documentation omits the accepted empty-string value

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:99`**

```text
`selector` is retired: only absence or `"off"` is accepted
```

The migration reference incorrectly labels a supported disabled legacy configuration as an init error.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:7085-7091`**

```text
let value = value.as_str().map(str::trim).ok_or_else(|| {
```

String values are trimmed before the compatibility check.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:7100-7108`**

```text
if !matches!(value, "" | "off") {
```

An empty string, including whitespace after trimming, is accepted silently rather than rejected.

### Independent adjudication

The retired selector's accepted-value claim is exclusive but incomplete. The actual parser trims strings and accepts the resulting empty value. This is a compatibility spelling, not a resurrected selector implementation or an argument to recommend new blank settings.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:7084-7108`**

```text
let value = value.as_str().map(str::trim).ok_or_else(|| {
...
if !matches!(value, "" | "off") {
```

Empty and whitespace-only strings pass, as does trimmed off; nonstrings and other strings fail.

**Required correction:** Say the retired selector accepts absence or a string that trims to off or empty for disabled compatibility. Recommend removing the key or using off; preserve refusal of activation-shaped values and the deterministic-recall behavior.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Added empty/whitespace-only strings to the retired selector's disabled compatibility forms after trimming; continued to recommend key removal or off and retained refusal of activation-shaped values.

**Validation:** Source assertion confirms the executable empty/off match in rpc_gateway.rs:7084-7108; the adjudicated parser trims strings before this check.

**Final review: pass.** The selector row documents absence and strings trimmed to off or empty, including whitespace, while recommending removal/off and preserving refusal of activation-shaped input.

**`docs/reference/configuration.mdx:104-104`**

```text
absence or a string that trims to `"off"` or empty is accepted as disabled compatibility
```

The compatibility admission set is now correct.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:7085-7100`**

```text
if !matches!(value, "" | "off") {
```

The preceding as_str().map(str::trim) and this branch jointly implement the stated rule.

## B-013: ConsolePolicy's documented field inventory omits its ui configuration field

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:827-838`**

```text
ConsolePolicy {
    require_app_auth: bool,
    read_only: bool,
    fetch_timeout_ms: Option<u64>,
}
```

Rust hosts cannot construct the complete advertised policy literal without an unexplained extra field and cannot discover where view configuration is attached.

**`meerkat-mobkit/src/decisions.rs:124-133`**

```text
#[serde(default, skip_serializing_if = "ConsoleUiConfig::is_default")]
    pub ui: ConsoleUiConfig,
```

The public Rust struct has a fourth field. It is also the link between the ConsoleUiConfig section and library-host policy construction.

**`meerkat-mobkit/src/decisions.rs:137-143`**

```text
ui: ConsoleUiConfig::default(),
```

The omitted field has a concrete default that belongs in the table.

### Independent adjudication

The schematic struct and adjoining field table both purport to enumerate ConsolePolicy but omit a real public field used at the documented experience boundary. Although the code block is a type sketch rather than a runnable literal, the missing inventory/linkage is still a concrete configuration-reference defect.

**`meerkat-mobkit/src/decisions.rs:124-143`**

```text
#[serde(default, skip_serializing_if = "ConsoleUiConfig::is_default")]
    pub ui: ConsoleUiConfig,
...
ui: ConsoleUiConfig::default(),
```

The field and its exact default are public and explicit.

**`meerkat-mobkit/src/runtime/console_ingress.rs:545-551,791-795`**

```text
let console_config = &console_policy.ui;
...
"console_config": console_config,
```

The field is the real source of the console_config experience projection, not unused internal state.

**Required correction:** Add ui: ConsoleUiConfig to the type sketch and a field-table row with ConsoleUiConfig::default(), identifying its view-level /console/experience projection and linking the existing console configuration section. Keep abbreviated value examples valid with ..ConsolePolicy::default() where applicable.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Added ui: ConsoleUiConfig to the schematic ConsolePolicy inventory and its table, including ConsoleUiConfig::default(), the view-level /console/experience projection and a link to the existing configuration section.

**Validation:** Source assertions confirm the public ui field and exact default in decisions.rs:124-143. The experience projection is checked against console_ingress.rs:545-551,791-795 in the adjudication. New consoleuiconfig anchor resolves. The separate console-guide value literal is a cross-scope request.

**Changed:** `docs/guides/console.mdx`.

Applied the additional owned-document propagation explicitly identified in audit-K coverage: completed the abbreviated ConsolePolicy value literal with ..ConsolePolicy::default(). The configuration-reference inventory remains scope B's responsibility.

**Validation:** Read audit-B/adjudication-B's confirmed B-013 correction and current decisions.rs:123-143. Verified the added struct update supplies the real fetch_timeout_ms and ui defaults without changing the two explicit values.

**Final review: pass.** The configuration type sketch and table include ui with the exact default and experience projection. The F-owned console-guide literal now uses a struct update, supplying both ui and fetch_timeout_ms. Both known occurrences are closed.

**`docs/reference/configuration.mdx:863-877`**

```text
ui: ConsoleUiConfig,
```

The complete field inventory is shown; the adjoining table documents the default and projection.

**`docs/guides/console.mdx:618-622`**

```text
..ConsolePolicy::default()
```

The cross-scope abbreviated value literal now supplies omitted fields.

**`meerkat-mobkit/src/decisions.rs:124-140`**

```text
ui: ConsoleUiConfig::default(),
```

The declared default matches the reference.

**`meerkat-mobkit/src/runtime/console_ingress.rs:550-550`**

```text
let console_config = &console_policy.ui;
```

The UI configuration is sourced from the documented field.

**`meerkat-mobkit/src/runtime/console_ingress.rs:791-795`**

```text
"console_config": console_config,
```

The experience response exposes the field as claimed.

**Final review: pass.** The F-owned value literal now uses struct-update defaults, supplying the previously missing fetch_timeout_ms and ui fields without altering its explicit auth/read-only intent. This review covers the requested literal propagation, not the separate B-owned configuration-reference inventory.

**`docs/guides/console.mdx:618-622`**

```text
ConsolePolicy {
    require_app_auth: true,   // the default; set false only for local development
    read_only: false,         // set true for view-only deployments
    ..ConsolePolicy::default()
}
```

The abbreviated value expression now accounts for every real public field.

**`meerkat-mobkit/src/decisions.rs:134-141`**

```text
impl Default for ConsolePolicy {
    fn default() -> Self {
        Self {
            require_app_auth: true,
            read_only: false,
            fetch_timeout_ms: None,
            ui: ConsoleUiConfig::default(),
```

The real Default implementation supplies the omitted fields. The public struct at 124-132 has exactly those four fields.

## B-014: The storage reference labels the live canonical mob database as merely reserved

**Severity:** high. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:528`**

```text
| event log | `event_log.sqlite3` (reserved) | — |
```

An operator following the file inventory can omit or discard this supposedly unused database and lose canonical mob definition/adopted-member/event history needed for restart.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11656-11671`**

```text
meerkat_mobkit::mob_composition_manifest::persistent_mob_storage(
                storage_layout.event_log_db(),
            )
```

The normal persistent SDK launch opens this locator as the canonical Meerkat mob store, including durable definition and adopted identity/event state.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:1710-1714`**

```text
meerkat_mobkit::mob_composition_manifest::persistent_mob_storage(
                layout.event_log_db(),
            )?;
```

The standalone persistent-session gateway uses the same database.

**`meerkat-mobkit/src/mob_composition_manifest.rs:630-634`**

```text
let storage = meerkat_mob::MobStorage::persistent(&path)?;
```

This is a real persistent MobStorage open, not a future placeholder or optional operational event-log projection.

**`meerkat-mobkit/src/storage_layout.rs:76-77`**

```text
pub const EVENT_LOG_DB_FILE_NAME: &str = "event_log.sqlite3";
```

The locator resolves to exactly the documented filename.

### Independent adjudication

The database is opened in both gateways' normal persistent composition, so 'reserved' is materially false and dangerous for storage inventories. I checked the shared open helper and its use in the bootstrap spec. The correction must retain the SDK's explicit ephemeral mob_storage exception rather than implying every launch always writes this database.

**`meerkat-mobkit/src/storage_layout.rs:77,466-467`**

```text
pub const EVENT_LOG_DB_FILE_NAME: &str = "event_log.sqlite3";
...
self.state_dir.join(EVENT_LOG_DB_FILE_NAME)
```

The documented filename is the live path authority's event_log_db locator.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11632-11671`**

```text
let pair = match meerkat_mobkit::mob_composition_manifest::persistent_mob_storage(
                storage_layout.event_log_db(),
            )
```

The normal persistent branch opens it, while the preceding explicit mob_storage_ephemeral branch is the opt-out. The failure text names lost adopted identity declarations and mob events.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:1710-1716`**

```text
meerkat_mobkit::mob_composition_manifest::persistent_mob_storage(
                layout.event_log_db(),
            )?;
        let mut spec = MobBootstrapSpec::new(definition, mob_storage, service)
```

The standalone persistent-session path feeds the opened database-backed storage into the actual mob bootstrap.

**`meerkat-mobkit/src/mob_composition_manifest.rs:630-634`**

```text
let storage = meerkat_mob::MobStorage::persistent(&path)?;
```

The shared helper performs a real persistent Meerkat MobStorage open; it is not a reserved path or the optional operational query projection.

**Required correction:** Rename the storage inventory row to canonical Meerkat mob storage and remove reserved from event_log.sqlite3. Explain that normal persistent gateway launches store canonical mob definition, events, and adopted-member state there, distinct from runtime_options.event_log's optional operational query projection. Mention the explicitly declared ephemeral SDK mob-storage exception if describing universality. Do not rename, migrate, or alter the database.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Replaced the reserved event-log row with canonical Meerkat mob storage at event_log.sqlite3. Explained definition/events/adopted-member persistence, distinction from runtime_options.event_log, and the SDK's explicit ephemeral mob_storage exception.

**Validation:** Source assertions confirm EVENT_LOG_DB_FILE_NAME and MobStorage::persistent in the live helper. Both persistent gateway call sites and the SDK opt-out branch were checked against the adjudicated contract; documented filename remains unchanged. No file migrations or runtime changes.

**Final review: pass.** The inventory now identifies event_log.sqlite3 as live canonical Meerkat mob storage, not a reserved operational projection. The added text distinguishes runtime_options.event_log and preserves the SDK's explicit ephemeral mob_storage opt-out.

**`docs/reference/configuration.mdx:557-566`**

```text
| canonical Meerkat mob storage | `event_log.sqlite3` | — |
```

The misleading reserved designation is gone.

**`meerkat-mobkit/src/mob_composition_manifest.rs:630-634`**

```text
let storage = meerkat_mob::MobStorage::persistent(&path)?;
```

The shared opener really creates persistent canonical mob storage.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:11632-11658`**

```text
storage_layout.event_log_db(),
```

The normal persistent SDK branch uses that locator; the preceding branch explicitly selects in-memory storage.

**`meerkat-mobkit/src/bin/mobkit_gateway.rs:1711-1716`**

```text
layout.event_log_db(),
```

The standalone persistent branch uses the same live locator.

## B-015: The documented stock shutdown horizon is twelve seconds shorter than the advertised contract

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:627-629`**

```text
The current 335-second horizon covers an already-admitted
provider callback, runtime event and mob drains, the final lease-release
callback, and bounded response-delivery/reaping margin.
```

An embedder using the reference's shorter timeout can kill a gateway during still-valid cleanup or provider lease-release work.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:7886-7889`**

```text
const GATEWAY_SHUTDOWN_HORIZON_MS: u64 = 347_000;
```

The stock gateway now allows 337 seconds of bounded phases plus ten seconds for response delivery and process reaping.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:13606`**

```text
"stdio_shutdown_horizon_ms": GATEWAY_SHUTDOWN_HORIZON_MS,
```

347,000ms is actually sent in the init response, not just an unused constant.

**`meerkat-mobkit/src/gateway_composition.rs:345-348`**

```text
pub const GATEWAY_RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(312);
```

The expanded runtime budget is consistent with the gateway's longer total horizon.

### Independent adjudication

The document hard-codes 335 seconds but the stock gateway advertises 347000ms. I followed the constant to the initialization response and checked the current runtime shutdown budget. The defect is the documented stock value, not the separate callback-deadline contracts.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:7886-7889`**

```text
const GATEWAY_SHUTDOWN_HORIZON_MS: u64 = 347_000;
```

The live constant is twelve seconds greater than the documented 335 seconds; adjacent code describes 337 seconds of gateway phases plus ten seconds of delivery/reaping margin.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:13602-13606`**

```text
"stdio_shutdown_horizon_ms": GATEWAY_SHUTDOWN_HORIZON_MS,
```

Clients receive that constant through the negotiated initialization handshake.

**`meerkat-mobkit/src/gateway_composition.rs:344-348`**

```text
pub const GATEWAY_RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(312);
```

The current shared runtime budget agrees with the expanded total; this is not a dead compatibility constant.

**Required correction:** Change the current stock shutdown horizon to 347 seconds and emphasize honoring the advertised stdio_shutdown_horizon_ms rather than hard-coding it. Preserve the distinct provider/SDK/wire deadlines and older-custom-gateway fallback semantics.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Updated the stock shutdown horizon to 347 seconds and explicitly required consuming advertised stdio_shutdown_horizon_ms rather than hard-coding it. Preserved provider/SDK/wire deadlines and older/custom EOF behavior.

**Validation:** Source assertions confirm GATEWAY_SHUTDOWN_HORIZON_MS = 347_000 and its actual init-response use. The public 120-second provider, 125-second Python completion and 130-second wire descriptions were left unchanged.

**Final review: pass.** The stock value is now 347 seconds with an explicit instruction to honor the advertised horizon. The distinct provider/SDK/wire deadlines and old/custom-gateway EOF fallback are preserved.

**`docs/reference/configuration.mdx:660-675`**

```text
Honor the advertised `stdio_shutdown_horizon_ms` rather than
hard-coding it. The current stock 347-second horizon
```

Both the numerical correction and negotiation caveat are present.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:7886-7889`**

```text
const GATEWAY_SHUTDOWN_HORIZON_MS: u64 = 347_000;
```

This is the current executable constant.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:13603-13606`**

```text
"stdio_shutdown_horizon_ms": GATEWAY_SHUTDOWN_HORIZON_MS,
```

The init handshake actually advertises that value.

## B-016: The profile-tool reference says no unknown-key check exists, but the pinned TOML parser now diagnoses it

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:439-442`**

```text
The table has no unknown-key check, so a
misspelled key (`comm = true`) is dropped and the flag it meant to set stays
`false`.
```

Readers are told this typo is undiagnosed and are not directed to the warning/typed diagnostic that now identifies their mistake.

**`meerkat-mobkit/Cargo.toml:63`**

```text
meerkat-mob = { version = "=0.8.40" }
```

The relevant dependency is the released 0.8.40 source, not an unpinned upstream branch.

**`https://docs.rs/crate/meerkat-mob/0.8.40/source/src/definition.rs:797-803`**

```text
/// Compare each `[profiles.<name>]` table against the keys its binding
/// declares: [`Profile::FIELD_NAMES`] (plus [`ToolConfig::FIELD_NAMES`] for the
/// `tools` sub-table)
```

Verified directly from https://static.crates.io/crates/meerkat-mob/meerkat-mob-0.8.40.crate in memory: parse_toml calls inspect_profile_keys, which inspects tool subkeys.

**`https://docs.rs/crate/meerkat-mob/0.8.40/source/src/definition.rs:962-970`**

```text
for unknown in &parsed.unknown_profile_keys {
            tracing::warn!(
```

from_toml emits a warning with the unknown key list; it does still ignore ordinary unknown keys, so the flag staying false remains accurate.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:10984`**

```text
let definition = MobDefinition::from_toml(mob_config_toml).unwrap_or_else(|e| {
```

The gateway uses the warning-emitting TOML ingress, not bare serde JSON deserialization.

### Independent adjudication

There is an important partial truth to preserve: raw ToolConfig serde remains permissive and comm still does not enable comms. However this section explicitly describes mob.toml ingestion, and the pinned gateway parser now inspects unknown tool keys and emits diagnostics. I independently downloaded the exact locked 0.8.40 crate in memory and checked its SHA-256 against Cargo.lock, rather than accepting a current-upstream assertion.

**`meerkat-mobkit/Cargo.toml:63`**

```text
meerkat-mob = { version = "=0.8.40" }
```

The dependency pin selects the source reviewed. The downloaded archive matched Cargo.lock checksum 5b517b514769cbfa7428a4f82051fd5b4d130f7624e99164d9e1066d9f9ce171.

**`https://docs.rs/crate/meerkat-mob/0.8.40/source/src/definition.rs:846-855,962-971,977-984`**

```text
.filter(|key| !ToolConfig::FIELD_NAMES.contains(&key.as_str()))
                        .map(|key| format!("tools.{key}"))
```

Verified from the checksum-matched crate archive: parse_toml calls inspect_profile_keys, which checks the tools subtable; from_toml iterates the result and warns.

**`https://docs.rs/crate/meerkat-mob/0.8.40/source/src/definition.rs:1403-1432`**

```text
fn parse_toml_warns_on_unknown_tools_keys_with_the_sub_table_location()
```

The pinned regression uses comm=true and asserts an UnknownProfileKey warning at profiles.worker.tools.comm while parsing succeeds.

**`https://docs.rs/crate/meerkat-mob/0.8.40/source/src/profile.rs:85-95`**

```text
`ToolConfig` has no `deny_unknown_fields`
```

The underlying serde representation remains permissive; the correction must distinguish the inspected TOML boundary from bare serde/JSON.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:10984`**

```text
let definition = MobDefinition::from_toml(mob_config_toml).unwrap_or_else(|e| {
```

The SDK gateway uses the warning-emitting TOML path.

**Required correction:** Replace the unconditional no-unknown-key-check statement with: raw ToolConfig serde ignores unknown keys, but MobDefinition::from_toml checks profile/tool keys and warns about ordinary ignored unknowns; parse_toml exposes typed diagnostics. A typo such as comm still does not enable comms. Do not imply all unknown keys are fatal or that JSON receives the TOML diagnostics.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Distinguished permissive raw ToolConfig serde from MobDefinition::from_toml unknown-profile/tool warnings and parse_toml typed diagnostics. Preserved the comm-versus-comms typo consequence and avoided claiming JSON diagnostics or universally fatal unknown keys.

**Validation:** Downloaded exact locked meerkat-mob 0.8.40 archive into memory, verified its SHA-256 against Cargo.lock, and checked inspect_profile_keys, ToolConfig::FIELD_NAMES, warning iteration and the unknown-tools-key regression. No dependency installation or archive extraction.

**Final review: pass.** The new wording correctly distinguishes permissive raw serde from inspected TOML, warning-emitting from_toml and parse_toml typed diagnostics. It retains the typo consequence and does not make all unknown fields fatal. Independently downloaded the exact locked 0.8.40 crate in memory, verified SHA-256, and read its implementation and unknown-tool-key fixture.

**`docs/reference/configuration.mdx:465-473`**

```text
Raw `ToolConfig` serde deserialization
ignores unknown keys, but `MobDefinition::from_toml` checks profile/tool keys
and warns about ordinary ignored unknowns
```

The final distinction matches the pinned implementation.

**`https://docs.rs/crate/meerkat-mob/0.8.40/source/src/definition.rs:846-855`**

```text
.filter(|key| !ToolConfig::FIELD_NAMES.contains(&key.as_str()))
```

Read from checksum-matched crates.io archive; the TOML inspector checks tool keys.

**`https://docs.rs/crate/meerkat-mob/0.8.40/source/src/definition.rs:962-984`**

```text
for unknown in &parsed.unknown_profile_keys {
```

from_toml emits warnings while parse_toml retains typed unknown-key data.

## B-017: Memory-mode guidance is pinned to an obsolete version and misses the autonomous console-human exception

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:940`**

```text
With the current Meerkat 0.8.32 pairing, ambient per-turn memory requires `runtime_mode = "turn_driven"`; remove this restriction after the Meerkat 0.8.33 carrier change lands.
```

The page instructs maintainers to remove a restriction solely on a version threshold that has already passed, while simultaneously claiming no autonomous memory path exists. Either reading can lead to the wrong runtime mode or incorrect expectations of generic dispatch injection.

**`meerkat-mobkit/Cargo.toml:59-64`**

```text
meerkat-core = { version = "=0.8.40" }
```

The claimed current pairing is false; this checkout already pins a later release.

**`meerkat-mobkit/src/identity_first/runtime.rs:8115-8120`**

```text
if local_human {
                    DeliveryPreparation::ConsoleHuman
                } else {
                    DeliveryPreparation::Work(memory_runtime_mode)
                },
```

Local console-human input has a separate delivery preparation boundary.

**`meerkat-mobkit/src/identity_first/runtime.rs:8521-8538`**

```text
DeliveryPreparation::Work(meerkat_mob::MobRuntimeMode::AutonomousHost)
```

Only generic autonomous work is skipped before recall. ConsoleHuman does not match that arm and can carry typed injected context even for autonomous members.

**`meerkat-mobkit/src/console_human_input_tests.rs:415-475`**

```text
async fn console_human_recall_stays_canonical_context_not_refreshed_or_paged_human_input() {
```

The regression fixture's harness uses autonomous_host (line 108) and verifies that configured recall reaches canonical InjectedContext messages for console human input.

**`docs/quickstart.mdx:196-200`**

```text
cannot receive ambient per-turn memory
```

The quickstart repeats the overbroad prohibition and should preserve the generic SDK/work caveat without excluding the supported console-human path.

### Independent adjudication

The release pairing note is obsolete and its universal autonomous-memory prohibition is contradicted by the local console-human delivery path. I checked the actual guard and the autonomous test harness. Conversely, the generic autonomous-work guard still exists after the named version threshold, so simply deleting all runtime-mode guidance would also be wrong.

**`meerkat-mobkit/Cargo.toml:59-64`**

```text
meerkat-core = { version = "=0.8.40" }
```

The checkout is not on the claimed current 0.8.32 pairing, and runtime behavior must be read from the local code rather than inferred from the promised 0.8.33 milestone.

**`meerkat-mobkit/src/identity_first/runtime.rs:8090-8098,8111-8123`**

```text
console_human.is_some() && !durable_spec_uses_external_binding(&entry.spec),
...
if local_human {
                    DeliveryPreparation::ConsoleHuman
                } else {
                    DeliveryPreparation::Work(memory_runtime_mode)
                },
```

Local canonical console-human input selects a distinct preparation mode; external bindings are not automatically included.

**`meerkat-mobkit/src/identity_first/runtime.rs:8506-8543`**

```text
DeliveryPreparation::Work(meerkat_mob::MobRuntimeMode::AutonomousHost)
```

Only this autonomous-work case skips ambient recall; ConsoleHuman reaches inject_for_turn. Steer returns before either injection path.

**`meerkat-mobkit/src/console_human_input_tests.rs:96-109,415-475`**

```text
runtime_mode = "autonomous_host"
...
.expect("configured recall must reach the canonical injected-context slot");
```

The regression constructs an autonomous member and asserts memory reaches the real canonical InjectedContext transcript slot for console-human input.

**Required correction:** Remove the obsolete 0.8.32 pairing and automatic-after-0.8.33 removal promise. State that generic SDK/work delivery to autonomous_host still skips ambient per-turn recall and turn_driven is needed for that path, while non-steer local canonical console-human input can carry configured typed injected context for autonomous_host. Narrow the corresponding quickstart sentence without removing the generic-work guard or claiming support for arbitrary external bindings.

### Changes and final verification

**Changed:** `docs/quickstart.mdx`, `docs/reference/configuration.mdx`.

Removed obsolete 0.8.32/0.8.33 release-future guidance. Preserved the generic SDK/work autonomous_host ambient-recall guard, added the non-steer local canonical console-human typed-context exception, and excluded arbitrary external bindings. Narrowed adjacent 'next send' wording to eligible turns and recall policy.

**Validation:** Read identity_first/runtime.rs:8495-8555 and checked its separate ConsoleHuman preparation versus Work(AutonomousHost), the steer early return and adjudicated local-binding condition. The autonomous console-human regression was inspected in the adjudication, not run. Quickstart/configuration syntax and local links pass.

**Changed:** `docs/sdks/python.mdx`.

Removed stale attribution of the pinned dependency to 0.8.32 without changing cache-policy guidance.

**Validation:** Applied independently adjudicated pin correction to the repeated SDK occurrence reported by fix-B/fix-D.

**Changed:** `docs/sdks/python.mdx`.

Replaced the blanket disabled Anthropic caching default with automatic on Anthropic API, Vertex, and Foundry, and disabled on Bedrock and Copilot. Explained that the latter backends reject request-wide automatic caching, framed the existing one-hour example as an explicit selection on a supported backend, and documented the disabled per-agent opt-out without cache_ttl. Preserved nested provider_tag, strict unknown-field validation, profile merging, resume propagation, and realm-reference qualifications.

**Validation:** Checked Cargo.lock:2251-2254 and the resolved meerkat-anthropic 0.8.40 src/runtime/mod.rs:60-91,617-627,647-653,709-717,731-741,768-775,804-811 plus src/client.rs:859-884. Verified the exact published archive in memory against Cargo.lock SHA-256 cf5bf2e5b50fb570d70dd225d4192e762af04de8977449c5aea0110692f335cd and confirmed the inspected cached runtime/client bytes match it. The backend selector, client builder configuration, request fallback, unsupported-automatic rejection, and disabled-plus-TTL rejection support the prose. Source/document assertions and MDX compilation passed.

**Final review: pass.** The primary memory-mode changes remain accurate. Independently rechecked the Python residual against the exact locked meerkat-anthropic 0.8.40 archive, SHA-256 cf5bf2e5b50fb570d70dd225d4192e762af04de8977449c5aea0110692f335cd. The reference now states automatic for AnthropicApi/Vertex/Foundry and disabled for Bedrock/Copilot, qualifies the one-hour example to supported backends, and documents the per-agent disabled override without cache_ttl. The request fallback and both rejection branches corroborate the new prose. B-R002 is resolved.

**`docs/reference/configuration.mdx:990-995`**

```text
Generic SDK/work delivery to `autonomous_host` still skips ambient per-turn
recall
```

The primary memory-mode restriction is correctly narrowed and followed by the console-human exception.

**`docs/quickstart.mdx:219-222`**

```text
Non-steer local
canonical console-human input can carry configured typed injected context even
for `autonomous_host`
```

The repeated quickstart prohibition is repaired.

**`meerkat-mobkit/src/identity_first/runtime.rs:8091-8096`**

```text
console_human.is_some() && !durable_spec_uses_external_binding(&entry.spec),
```

The exception is local and does not silently include external bindings.

**`meerkat-mobkit/src/identity_first/runtime.rs:8505-8534`**

```text
DeliveryPreparation::Work(meerkat_mob::MobRuntimeMode::AutonomousHost)
```

Only this work case skips recall after the steer early return; ConsoleHuman reaches injection.

**`docs/sdks/python.mdx:718-722`**

```text
dependency defaults to `automatic` on Anthropic API, Vertex, and Foundry, and to
`disabled` on Bedrock and Copilot.
```

The repeated current-default claim now preserves the exact pinned backend distinction.

**`docs/sdks/python.mdx:734-735`**

```text
To opt out per agent, set `provider_tag.cache_control` to `"disabled"` and omit
`cache_ttl`.
```

The added opt-out avoids the disabled-plus-explicit-TTL refusal.

**`https://docs.rs/crate/meerkat-anthropic/0.8.40/source/src/runtime/mod.rs:69-91`**

```text
AnthropicBackendKind::AnthropicApi
        | AnthropicBackendKind::Vertex
        | AnthropicBackendKind::Foundry => true,
```

The checksum-matched pin selects Automatic on these supported backends and Disabled on Bedrock/Copilot.

**`https://docs.rs/crate/meerkat-anthropic/0.8.40/source/src/runtime/mod.rs:732-741`**

```text
.default_cache_control(default_cache_control_for_backend(backend_kind))
```

The native API-key production path actually applies the selector; it is not merely an unused helper.

**`https://docs.rs/crate/meerkat-anthropic/0.8.40/source/src/client.rs:859-884`**

```text
.unwrap_or(self.default_cache_control);
```

Omitted overrides inherit the backend default. The following branches reject disabled with explicit TTL and automatic on unsupported backends, matching both new qualifications.

**Final review: pass.** Independently re-reviewed the residual fix against the exact Cargo.lock-resolved meerkat-anthropic 0.8.40 implementation. The final text correctly defaults to Automatic on Anthropic API, Vertex, and Foundry, and Disabled on Bedrock and Copilot; it states that the latter reject request-wide Automatic. The example is now explicitly limited to a supported backend and selects a one-hour TTL rather than claiming all backends require an opt-in. The disabled opt-out correctly omits cache_ttl because the actual request builder rejects a TTL combined with Disabled. Verified the locally cached crate archive against Cargo.lock SHA-256 and verified that both inspected provider source files match that archive. The previous failure and its resolution are preserved in review_history.

**`docs/sdks/python.mdx:718-723`**

```text
dependency defaults to `automatic` on Anthropic API, Vertex, and Foundry, and to
`disabled` on Bedrock and Copilot. Bedrock and Copilot do not support request-wide
automatic caching and reject an explicit `automatic` policy. On a supported
backend, this example explicitly selects automatic caching with a one-hour TTL:
```

The final paragraph preserves the exact current backend-dependent default, unsupported-backend rejection, and example prerequisite.

**`docs/sdks/python.mdx:734-735`**

```text
To opt out per agent, set `provider_tag.cache_control` to `"disabled"` and omit
`cache_ttl`.
```

The new opt-out guidance does not accidentally retain the example's invalid disabled-plus-TTL combination.

**`meerkat-mobkit/Cargo.toml:59-64`**

```text
meerkat-client = { version = "=0.8.40" }
```

The current library consumes the exact 0.8.40 provider family; release-neutral prose is verified against this pin rather than a moving upstream branch.

**`Cargo.lock:2251-2254`**

```text
name = "meerkat-anthropic"
version = "0.8.40"
```

The exact resolved Anthropic runtime is 0.8.40. The meerkat-client 0.8.40 lock entry at 2322-2332 depends on it. The locked crate checksum is cf5bf2e5b50fb570d70dd225d4192e762af04de8977449c5aea0110692f335cd.

**`(operator-retained path)/meerkat-anthropic-0.8.40/src/runtime/mod.rs:69-91`**

```text
fn default_cache_control_for_backend(backend: AnthropicBackendKind) -> AnthropicCacheControlPolicy {
    if backend_supports_automatic_cache_control(backend) {
        AnthropicCacheControlPolicy::Automatic
    } else {
        AnthropicCacheControlPolicy::Disabled
    }
}
```

This is the authenticated pinned registry source's operative default. The adjacent exhaustive match is true only for AnthropicApi/Vertex/Foundry and false for Bedrock/Copilot. Runtime construction passes this selector and support flag into clients, including runtime/mod.rs:709-717.

**`(operator-retained path)/meerkat-anthropic-0.8.40/src/client.rs:859-884`**

```text
        let cache_control = anthropic_tag(request)
            .and_then(|tag| tag.cache_control)
            .unwrap_or(self.default_cache_control);
```

The request builder applies the backend default when no override exists. The same reviewed range rejects cache_ttl plus Disabled and rejects Automatic on unsupported backends; all three semantics now match the page.

## B-018: Memory injection limits are stated as lifetime per-session limits although compaction resets them

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:952`**

```text
60 KiB cumulative per delivered session, with records offered at most once per session (cross-turn dedup).
```

Hosts can incorrectly reason about maximum prompt volume and suppression of repeated memory across a long-lived compacting session.

**`meerkat-mobkit/src/memory/coordinator.rs:440-450`**

```text
pub fn on_session_compacted(&self, session_key: &str) {
        self.session_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_key);
```

Compaction removes the session's dedup set and cumulative byte accounting.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12892-12903`**

```text
injector.on_session_compacted(session);
```

The stock SQLite gateway installs this CompactionResetSink unconditionally for the memory stack, not only when the Distiller is enabled.

**`meerkat-mobkit/src/memory/coordinator.rs:2424-2479`**

```text
"post-compaction turns may re-inject: {}"
```

The regression test explicitly proves that the same record can be injected again into the same session after compaction.

### Independent adjudication

The 60KiB and once-per-session wording reads as a lifetime limit, but the same session can receive a record again after its observed compaction resets accounting. I followed the reset from the actual event sink through stock gateway wiring to the coordinator and checked the reinjection regression. This automatic wiring should be stated for the composed stack, not promised for an arbitrary custom injector that never forwards compaction events.

**`meerkat-mobkit/src/memory/coordinator.rs:440-450`**

```text
pub fn on_session_compacted(&self, session_key: &str) {
        self.session_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_key);
```

The entire session's cross-turn dedup and byte accounting is removed; compaction does not merely reduce a counter.

**`meerkat-mobkit/src/memory/taint.rs:868-875`**

```text
if matches!(envelope.payload, AgentEvent::CompactionCompleted { .. })
```

CompactionResetSink forwards the authoritative session ID from a session-source CompactionCompleted event.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12892-12904`**

```text
injector.on_session_compacted(session);
```

The stock memory stack installs this sink independently of whether the Distiller is enabled.

**`meerkat-mobkit/src/memory/coordinator.rs:2424-2479`**

```text
async fn compaction_reset_clears_budget_and_dedup_and_allows_reinjection()
```

The regression first confirms same-session dedup, resets session-a, verifies session-b survives, and then verifies the original record is reinjected into session-a.

**Required correction:** Describe cumulative budgeting and cross-turn dedup as session-keyed between compaction resets, not lifetime-per-session limits. Explain that the composed memory stack forwards CompactionCompleted to clear both so records may be offered again. Retain the unchanged per-record/per-assembly caps and distinguish this preparation-time accounting from delivered transcript context. Custom embedders must forward the reset when composing their own observation path.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Made the cumulative 60-KiB accounting and cross-turn dedup session-keyed between compaction resets rather than lifetime limits. Documented composed CompactionCompleted reset wiring, possible reinjection and the corresponding custom-embedder obligation, preserving preparation versus delivered-transcript semantics and existing per-record/per-assembly caps.

**Validation:** Source assertions confirm on_session_compacted removes session state and rpc_gateway forwards compaction to it. Compared memory/taint.rs:868-875 and coordinator.rs:2424-2479 in the adjudication; source-contract verification only, not a live compaction run.

**Changed:** `docs/reference/configuration.mdx`.

Scoped automatic CompactionCompleted reset forwarding to rpc_gateway instead of promising it for every composed memory stack. Explicitly documented that the current standard Rust persistent_agent_memory_stack builder does not install the reset sink and that native/custom compositions need equivalent sink-to-injector reset wiring. Preserved cumulative-byte and cross-turn-dedup reset semantics, reinjection after reset, per-record/per-assembly caps, and preparation-versus-delivery accounting.

**Validation:** Read meerkat-mobkit/src/bin/rpc_gateway.rs:12892-12913, src/unified_runtime/builder.rs:1456-1482, the complete sink assembly in src/memory_wiring.rs:192-295, src/memory/taint.rs:852-875, src/identity_first/agent_memory.rs:551-555, and src/memory/coordinator.rs:440-450,2424-2479. The gateway adds CompactionResetSink; the native builder passes unaugmented stack.sinks. A scoped source search found no other production injector reset caller. Read the existing reinjection regression without running it. Source/document assertions and MDX compilation passed.

**Final review: pass.** The residual paragraph now names rpc_gateway as the automatically wired stack and explicitly excludes the current standard Rust persistent_agent_memory_stack builder. Native/custom hosts are instructed to connect CompactionResetSink to the injector reset before assuming resets. Re-read the gateway callback, native observer construction and complete memory_wiring sink assembly; the boundary matches. Budget/dedup reset, reinjection after reset, per-assembly caps and preparation-versus-delivery distinctions remain intact. B-R003 is resolved.

**`docs/reference/configuration.mdx:1003-1003`**

```text
Rust builders can use `.persistent_agent_memory_stack(config, engines)`
```

The section explicitly includes the standard native full-stack composition.

**`docs/reference/configuration.mdx:1011-1015`**

```text
The `rpc_gateway` memory stack forwards session `CompactionCompleted` events
through `CompactionResetSink` to clear both cumulative byte accounting and
cross-turn dedup for that session.
```

Automatic reset wiring is now limited to the actual gateway composition.

**`docs/reference/configuration.mdx:1017-1021`**

```text
The current standard Rust `.persistent_agent_memory_stack(...)` builder does
not install this reset sink
```

The native full-stack limitation is explicit, not hidden behind a generic custom-embedder caveat.

**`meerkat-mobkit/src/memory/coordinator.rs:445-449`**

```text
.remove(session_key);
```

When invoked, reset genuinely clears both accounting and dedup.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12896-12903`**

```text
injector.on_session_compacted(session);
```

The SDK gateway explicitly adds the reset callback.

**`meerkat-mobkit/src/unified_runtime/builder.rs:1475-1482`**

```text
crate::spawn_member_event_observer(runtime.mob_handle(), stack.sinks),
```

The standard library full-stack observer receives unaugmented stack.sinks.

**`meerkat-mobkit/src/memory_wiring.rs:192-192`**

```text
let mut sinks: Vec<Arc<dyn MemberAgentEventSink>> = vec![Arc::new(taint.clone())];
```

Reading the complete construction through line 295 shows only this sink plus optional DistillerTriggers and StewardTriggers; no reset sink is composed.

## B-019: The local assertion-ledger snapshot filename is documented with the wrong suffix rule

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/reference/configuration.mdx:913`**

```text
the whole state to `<state_path>.tmp`, then renamed over the file
```

Operators inspecting an interrupted ledger write or configuring file monitoring/cleanup are directed to a filename the implementation never writes.

**`meerkat-mobkit/src/runtime/memory.rs:128-132`**

```text
let tmp_path = self.state_path.with_extension("tmp");
```

with_extension replaces the existing extension instead of appending .tmp. The documented default memory-ledger-state.json is staged through memory-ledger-state.tmp, not memory-ledger-state.json.tmp.

### Independent adjudication

The documented suffix concatenation is observably different from the Rust path operation when the configured state file has an extension, including the documented default. This is a narrow filename-contract correction; the write/rename sequence itself is not being changed.

**`meerkat-mobkit/src/runtime/memory.rs:122-134`**

```text
let tmp_path = self.state_path.with_extension("tmp");
...
fs::rename(&tmp_path, &self.state_path)
```

with_extension replaces the existing extension, so memory-ledger-state.json stages through memory-ledger-state.tmp rather than memory-ledger-state.json.tmp.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6719-6720`**

```text
const MEMORY_LEDGER_STATE_FILE: &str = "memory-ledger-state.json";
```

The stock filename actually has the .json extension affected by the distinction.

**Required correction:** Replace <state_path>.tmp with a sibling path formed by replacing the final extension with .tmp (state_path.with_extension("tmp")); give memory-ledger-state.tmp for the default JSON filename, followed by rename over the original state path.

### Changes and final verification

**Changed:** `docs/reference/configuration.mdx`.

Corrected assertion-ledger staging from suffix appending to state_path.with_extension("tmp"), including memory-ledger-state.json -> memory-ledger-state.tmp before rename over the original.

**Validation:** Source assertions confirm with_extension("tmp") and fs::rename in runtime/memory.rs:122-134. Only filename documentation changed; no persistence algorithm change.

**Final review: pass.** The staging path now uses replacement of the final extension, with the correct memory-ledger-state.tmp example, then rename over the original. No persistence behavior is changed or newly promised.

**`docs/reference/configuration.mdx:961-961`**

```text
`memory-ledger-state.json` stages through `memory-ledger-state.tmp`
```

The append-versus-replace error is corrected.

**`meerkat-mobkit/src/runtime/memory.rs:128-133`**

```text
let tmp_path = self.state_path.with_extension("tmp");
```

The implementation replaces the extension before writing and renaming.

## Independent scope checks

> [
>   "Read audit-brief.md, audit-scopes.json, and every finding in audit-B.json; independently inspected the cited document wording and executable implementation, with additional counterexample/branch tracing.",
>   "Initial and final checkout baseline checked with git rev-parse HEAD; initial git status --short was clean.",
>   "Attempted PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=sdk/python python3 -m pytest sdk/python/tests/test_identity_first_builder_dispatcher.py -q -p no:cacheprovider -k 'test_roster_callback_receives_the_gateway_context or test_roster_callback_tolerates_a_bare_context'; unavailable because the system Python has no pytest.",
>   "Alternative validation passed: a python3 -B asyncio exercise imported the real SDK CallbackDispatcher and RosterContext, asserted definition/previous-identities preservation, and asserted both legacy empty-envelope defaults. No dependency install, bytecode, cache, or temporary files were used.",
>   "Downloaded meerkat-mob 0.8.40 crate bytes in memory, verified SHA-256 against Cargo.lock (5b517b514769cbfa7428a4f82051fd5b4d130f7624e99164d9e1066d9f9ce171), and independently read TOML key checking, warning emission, permissive ToolConfig serde, and the exact comm-typo regression fixture.",
>   "Rust and TypeScript regression tests cited above were inspected as source, not executed. No gateway, LLM, HTTP listener, Docker deployment, or full build was launched."
> ]

## Final scope checks

> [
>   "Read audit-brief.md, audit-scopes.json, audit-B/adjudication-B/fixes-B, audit-K/adjudication-K, fixes-D/fixes-F, fixes-coordination, and applicable K-005 entries in fixes-C/fixes-E. Read all six B-owned pages and the complete three-file B diff; inspected cross-scope repeated fixes and SDK documentary diffs.",
>   "Independently inspected executable parser, callback, lifecycle, storage, auth, warning, watchdog and memory-composition paths. Did not accept stale code comments or prior reports as implementation authority.",
>   "git diff --check passed for all six B pages and the Rust SDK, Python SDK and console-guide propagation pages. python3 -B scripts/check-conflict-markers passed for all six B pages.",
>   "Read-only Python document validation passed: six regular/non-symlink B pages, balanced fences, 7 Python AST snippets, 7 TOML snippets, 5 JSON snippets, and 42 local page/anchor links. Introduction, architecture and decisions are byte-identical to HEAD.",
>   "MDX compilation with the coordinator's already-installed @mdx-js/mdx and remark-frontmatter passed 9/9 B-owned and propagation pages. No dependencies installed or generated files written.",
>   "PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=sdk/python python3 -B: actual CallbackDispatcher preserved populated definition/prior identities and tolerated empty/absent context; actual Python builder accepted console_auth_required(False) and delegate retirement None/0/300. Confirmed no Python max_sessions setter.",
>   "NODE_DISABLE_COMPILE_CACHE=1 node --experimental-strip-types: actual TypeScript parseRosterContext passed populated, missing/null definition and empty-envelope probes.",
>   "Reproduced B-R001 by asserting the only standalone warn_on_non_loopback_bind call occurs after decisions.console.require_app_auth=true and receives &decisions. Its classifier emits 'console app auth is enforced' from the final state.",
>   "Downloaded exact locked crates into memory only and verified SHA-256 against Cargo.lock. meerkat-mob 0.8.40: 5b517b514769cbfa7428a4f82051fd5b4d130f7624e99164d9e1066d9f9ce171. Inspected TOML unknown-tool diagnostics and their regression fixture.",
>   "Verified exact meerkat-core 0.8.40 archive checksum 003fbea1ea9bc5cf094cf1fbf263ac0b23934822255a4c973cf3d48750832244 and meerkat-anthropic 0.8.40 checksum cf5bf2e5b50fb570d70dd225d4192e762af04de8977449c5aea0110692f335cd. The Anthropic runtime selector, production builder use and request fallback independently prove B-R002.",
>   "Read complete attach_memory_engines sink assembly and native full-stack observer installation; scoped rg of CompactionResetSink/on_session_compacted across src confirms the only production reset invocation is rpc_gateway, proving B-R003.",
>   "Residual re-review 2026-09-22: read fixes-review-residuals.json; independently re-read all three failed-item source paths and revised prose, plus changed B-007 configuration/RPC wording and both K-005 SDK reference occurrences. All three historical regressions are resolved; historical failure records and their resolutions are retained in review_history.",
>   "Residual re-review: re-fetched exact locked meerkat-anthropic 0.8.40 entirely in memory and verified checksum cf5bf2e5b50fb570d70dd225d4192e762af04de8977449c5aea0110692f335cd. Re-read backend selector, production native API builder use, request fallback, unsupported-automatic rejection and disabled-plus-TTL rejection.",
>   "Residual re-review: git diff --check passed for the five relevant changed MDX files. Current MDX compilation passed 11/11 B-owned and B/K propagation pages using existing dependencies with no compiled-file writes. Refreshed changed B-003/B-005/B-007/B-017/B-018 citations; current-item quotes are verified separately from intentionally historical quotations.",
>   "No repository edits, commits, staging, delegation, package installation, gateway/LLM/provider invocation, or runtime fixture writes. Only this requested review artifact was created."
> ]

## Review feedback and resolution history

Earlier review failures are retained here; the per-item dispositions above reflect the final re-review rather than erasing the feedback.

```json
[
  {
    "phase": "wave4-initial",
    "date": "2026-09-22",
    "summary": {
      "items": 19,
      "pass": 16,
      "fail": 3,
      "regressions": 3
    },
    "historical_evidence_note": "The following failure observations and document quotations describe the text before the residual fixes. They are intentionally preserved as historical feedback rather than presented as current defects.",
    "failed_items": [
      {
        "id": "B-003",
        "verdict": "fail",
        "reason": "The principal correction is accurate: top-level standalone JWT/OIDC auth exists, and the early exposure gate still requires allow_remote. However, the fix introduced a false operational-log claim. The WARN is not produced from that early gate's open snapshot: mobkit_gateway applies auth first and passes the final decisions to its only warn_on_non_loopback_bind call. Deployment lines 163-166 must describe the final serving posture, not the admission-time posture. See B-R001."
      },
      {
        "id": "B-017",
        "verdict": "fail",
        "reason": "The B-owned quickstart/configuration correction passes: generic autonomous work remains excluded, local canonical console-human input is qualified, and steer/external-binding caveats survive. The Python SDK occurrence is not fully closed: replacing '0.8.32 (the pinned release)' with 'the pinned Meerkat dependency' preserves a false current-default claim. Exact locked meerkat-anthropic 0.8.40 defaults to automatic on AnthropicApi/Vertex/Foundry and disabled on Bedrock/Copilot. The production client builder consumes that selector. See B-R002; simply deleting the obsolete version was insufficient."
      },
      {
        "id": "B-018",
        "verdict": "fail",
        "reason": "The session-keyed budget/dedup correction and rpc_gateway reset behavior are accurate. However, 'The composed memory stack forwards' is too broad in a section that explicitly introduces the Rust persistent_agent_memory_stack builder. That stock library full-stack path starts an observer with stack.sinks; attach_memory_engines adds taint, optional Distiller, and optional Steward sinks, but no CompactionResetSink. The only production injector reset call is in rpc_gateway. The custom-observation-path caveat does not distinguish this standard Rust builder. Narrow automatic wiring to rpc_gateway and disclose the native full-stack limitation rather than changing runtime code. See B-R003."
      }
    ],
    "regressions": [
      {
        "id": "B-R001",
        "related_id": "B-003",
        "severity": "medium",
        "title": "New exposure-warning prose mistakes the pre-auth gate state for the final logged state",
        "doc": {
          "path": "docs/guides/deployment.mdx",
          "lines": "163-166",
          "quote": "bound address and the admission-time auth posture. On `mobkit_gateway` this\nreflects the open pre-bootstrap state, not any later auth overlay."
        },
        "evidence": [
          {
            "path": "meerkat-mobkit/src/bin/mobkit_gateway.rs",
            "lines": "2141-2148",
            "quote": "decisions.console.require_app_auth = true;",
            "explanation": "Auth is overlaid first."
          },
          {
            "path": "meerkat-mobkit/src/bin/mobkit_gateway.rs",
            "lines": "2171-2175",
            "quote": "http_binding.local_addr(),\n        &decisions,",
            "explanation": "The sole warning call then consumes that overlaid state."
          },
          {
            "path": "meerkat-mobkit/src/gateway_composition.rs",
            "lines": "639-644",
            "quote": "ConsoleAuthPosture::Enforced => \"console app auth is enforced\",",
            "explanation": "The warning names authenticated posture when auth is configured."
          }
        ],
        "impact": "Operators are told to disregard the actual authenticated WARN text and can incorrectly interpret an open-console warning as harmless pre-bootstrap logging.",
        "correction": "Restore the distinction: the early standalone gate checks the open default and still needs allow_remote, but the post-bootstrap non-loopback WARN uses the final serving decision state, including any auth_config overlay. Remove 'admission-time' and the claim that this WARN always reflects the pre-bootstrap open state."
      },
      {
        "id": "B-R002",
        "related_id": "B-017",
        "severity": "medium",
        "title": "Removing the old dependency version leaves a false current Anthropic caching default",
        "doc": {
          "path": "docs/sdks/python.mdx",
          "lines": "717-719",
          "quote": "the pinned Meerkat dependency defaults it to\n`disabled` on every backend"
        },
        "evidence": [
          {
            "path": "https://docs.rs/crate/meerkat-anthropic/0.8.40/source/src/runtime/mod.rs",
            "lines": "69-91",
            "quote": "if backend_supports_automatic_cache_control(backend) {\n        AnthropicCacheControlPolicy::Automatic\n    } else {\n        AnthropicCacheControlPolicy::Disabled\n    }",
            "explanation": "Checksum-verified locked source maps AnthropicApi, Vertex and Foundry to automatic; Bedrock and Copilot to disabled."
          },
          {
            "path": "https://docs.rs/crate/meerkat-anthropic/0.8.40/source/src/runtime/mod.rs",
            "lines": "736-741",
            "quote": ".default_cache_control(default_cache_control_for_backend(backend_kind))",
            "explanation": "The ordinary Anthropic API client construction uses this default."
          },
          {
            "path": "https://docs.rs/crate/meerkat-anthropic/0.8.40/source/src/client.rs",
            "lines": "859-861",
            "quote": ".unwrap_or(self.default_cache_control);",
            "explanation": "An omitted request/profile cache-control override actually falls back to the selected backend default."
          }
        ],
        "impact": "The SDK guide misstates a billing-affecting default, implies an explicit opt-in is universally required, and suggests automatic caching without its unsupported-backend qualification.",
        "correction": "Describe the actual pinned capability-derived default: automatic for AnthropicApi/Vertex/Foundry, disabled for Bedrock/Copilot. Present the existing automatic/1h example as an explicit override on a supported backend, not a universal opt-in prerequisite; mention cache_control='disabled' as the per-agent opt-out. Alternatively remove the default claim and link to a verified backend-specific contract, but do not retain 'disabled on every backend'."
      },
      {
        "id": "B-R003",
        "related_id": "B-018",
        "severity": "medium",
        "title": "Automatic compaction reset is promised more broadly than the shipped gateway-only wiring",
        "doc": {
          "path": "docs/reference/configuration.mdx",
          "lines": "1011-1015",
          "quote": "The composed memory stack forwards session `CompactionCompleted` events to\nclear both cumulative byte accounting and cross-turn dedup for that session."
        },
        "evidence": [
          {
            "path": "meerkat-mobkit/src/bin/rpc_gateway.rs",
            "lines": "12896-12903",
            "quote": "injector.on_session_compacted(session);",
            "explanation": "The automatic reset is explicitly added by rpc_gateway."
          },
          {
            "path": "meerkat-mobkit/src/unified_runtime/builder.rs",
            "lines": "1479-1482",
            "quote": "crate::spawn_member_event_observer(runtime.mob_handle(), stack.sinks),",
            "explanation": "The native full-stack builder passes the unaugmented sinks from attach_memory_engines."
          },
          {
            "path": "meerkat-mobkit/src/memory_wiring.rs",
            "lines": "192-295",
            "quote": "sinks.push(Arc::new(DistillerTriggers::new(engine.clone())));",
            "explanation": "The complete sink assembly has taint plus optional Distiller and Steward; no CompactionResetSink or injector-reset callback. A scoped source search finds no production on_session_compacted invocation outside rpc_gateway."
          }
        ],
        "impact": "Rust embedders using the standard persistent_agent_memory_stack builder can expect reinjection after compaction even though that composition does not wire the reset. They are not necessarily composing a custom observation path, so the existing caveat does not clearly cover them.",
        "correction": "Name rpc_gateway as the automatically wired stack. Explain that native/custom compositions, including the current standard Rust full-stack builder, need an equivalent CompactionResetSink/injector callback before assuming compaction resets their session accounting. Preserve the actual reset semantics and do not alter runtime code in this documentation task."
      }
    ],
    "resolution": {
      "phase": "wave4-residual-rereview",
      "date": "2026-09-22",
      "fix_report": "fixes-review-residuals.json",
      "items": [
        {
          "id": "B-003",
          "regression_id": "B-R001",
          "previous_verdict": "fail",
          "verdict": "pass",
          "reason": "Final-serving auth WARN is now distinguished from early open-state admission; direct call-order verification passed."
        },
        {
          "id": "B-017",
          "regression_id": "B-R002",
          "previous_verdict": "fail",
          "verdict": "pass",
          "reason": "Python now reflects the exact meerkat-anthropic 0.8.40 backend selector, unsupported-automatic rejection and disabled-without-TTL opt-out; checksum-verified archive re-read passed."
        },
        {
          "id": "B-018",
          "regression_id": "B-R003",
          "previous_verdict": "fail",
          "verdict": "pass",
          "reason": "Automatic reset is scoped to rpc_gateway and the standard Rust builder's missing sink is explicit; gateway/native composition comparison passed."
        }
      ],
      "additional_rechecks": [
        {
          "id": "B-007",
          "verdict": "pass",
          "reason": "Configuration and RPC reference now both make caller risk_tier optional only when matching SDK/stdio policy supplies it; direct rewrite/parser inspection passed."
        },
        {
          "id": "K-005",
          "verdict": "pass",
          "reason": "Both Python and TypeScript SDK reference pages now carry the complete call-site-dependent context qualification; prior quickstart, RPC, roster and SDK-source evidence remains valid."
        }
      ],
      "remaining_regressions": 0
    }
  }
]
```
