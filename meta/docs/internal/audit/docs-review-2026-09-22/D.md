# D: Python, TypeScript and Rust SDK documentation

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

## D-001: Python observation examples read attributes absent from the yielded event types

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/python.mdx:223-231; also 42-43`**

```text
async for event in handle.subscribe_agent("agent-1"):
    print(event.event_type, event.data)

# Stream mob-wide events (all agents)
async for event in handle.subscribe_mob():
    print(event.source, event.data)
```

Every displayed observation loop raises AttributeError on its first event instead of printing or processing it, including the quickstart once connection and membership prerequisites are satisfied.

**`sdk/python/meerkat_mobkit/runtime.py:3689-3699`**

```text
async def subscribe_agent(self, member_id: str) -> AsyncIterator[AgentEvent]:
        """Stream events for one agent. Pure observation."""
        bridge = self._runtime.sse_bridge()
        async for event in bridge.agent_events(member_id):
            yield AgentEvent.from_sse(event, agent_id=member_id)
```

This public method yields AgentEvent, not a raw SseEvent or EventEnvelope. The adjacent subscribe_mob similarly yields MobEvent.from_sse, ruling out a different runtime event shape.

**`sdk/python/meerkat_mobkit/events.py:219-228; 265-276`**

```text
member_id: str
    event: Event
    timestamp_ms: int = 0
```

MobEvent has member_id/event/timestamp_ms, not source/data. AgentEvent has event_type/event, not data; both are frozen slots dataclasses, with no compatibility properties. Direct source imports reproduced AttributeError for AgentEvent.data and MobEvent.source/data. This is independent of missing quickstart member creation and HTTP authorization.

### Independent adjudication

Tried to disprove this by distinguishing the raw SSE envelope from the public subscription return types and looking for compatibility attributes. The public methods convert SseEvent to frozen, slotted AgentEvent/MobEvent objects; neither conversion exposes the documented attributes. Executed the real MobHandle subscriptions with a synthetic raw-event bridge: AgentEvent.data, MobEvent.source and MobEvent.data each raised AttributeError, whereas event.event and event.member_id worked. UnknownEvent.data is an inner-event field and does not rescue the outer wrapper example.

**`docs/sdks/python.mdx:223-231`**

```text
    print(event.event_type, event.data)
```

The observation section uses the invalid agent property; the same expression occurs at quickstart line 43. The mob loop at line 231 also uses source/data.

**`sdk/python/meerkat_mobkit/runtime.py:3689-3699`**

```text
            yield AgentEvent.from_sse(event, agent_id=member_id)
```

The adjacent subscribe_mob method yields MobEvent.from_sse(event). These are not raw SSE event envelopes.

**`sdk/python/meerkat_mobkit/events.py:219-228`**

```text
    member_id: str
    event: Event
    timestamp_ms: int = 0
```

MobEvent's public fields are member_id/event/timestamp_ms, with no source/data aliases.

**`sdk/python/meerkat_mobkit/events.py:265-276`**

```text
    event_type: str
    event: Event
```

AgentEvent exposes event_type/event. Direct execution of the actual subscription methods verified both the failures and proposed replacement fields without network traffic.

**Required correction:** In both Python agent subscription examples, use print(event.event_type, event.event). In the mob subscription example, use print(event.member_id, event.event). Keep these distinct from raw SseEvent.data and structural MobStructuralEvent.data.

### Changes and final verification

**Changed:** `docs/sdks/python.mdx`.

Both agent subscription examples print event.event_type and event.event; the mob example prints event.member_id and event.event. Structural-event e.data usage remains unchanged.

**Validation:** Executed the exact quickstart and both observation loops with the real Python MobHandle subscription wrappers and typed event parsers against in-memory RPC/HTTP or raw SSE boundaries. All corrected attribute accesses and prints succeeded.

**Final review: pass.** Both agent examples now access event_type/event, and the mob example accesses member_id/event. Independently executed the exact quickstart and observation code through the real MobHandle wrappers and typed parsers, with only external transport boundaries replaced. All three corrected prints succeeded. Structural MobStructuralEvent.data examples remain distinct and unchanged.

**`docs/sdks/python.mdx:77-79`**

```text
        async for event in handle.subscribe_agent("agent-1"):
            print(event.event_type, event.event)
```

The quickstart no longer reads the nonexistent AgentEvent.data field.

**`docs/sdks/python.mdx:268-275`**

```text
async for event in handle.subscribe_mob():
    print(event.member_id, event.event)
```

The mob example uses the actual attributed wrapper fields.

**`sdk/python/meerkat_mobkit/events.py:219-228`**

```text
    member_id: str
    event: Event
    timestamp_ms: int = 0
```

The MobEvent definition independently matches the corrected example.

**`sdk/python/meerkat_mobkit/runtime.py:3689-3699`**

```text
            yield AgentEvent.from_sse(event, agent_id=member_id)
```

The actual public wrapper returns AgentEvent; the adjacent mob wrapper returns MobEvent. In-memory execution exercised these implementations, not substitute event classes.

## D-002: The Python quickstart opens a protected SSE stream without an authentication opt-out or token

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/python.mdx:34-43`**

```text
async def main():
    async with await MobKit.builder().mob("config/mob.toml").gateway("/path/to/rpc_gateway").build() as rt:
        handle = rt.mob_handle()

        # Send a message (comms)
        await handle.send("agent-1", "Hello")

        # Watch what happens (observation)
        async for event in handle.subscribe_agent("agent-1"):
```

After the already-known member-creation issue is corrected, the quickstart still terminates with urllib HTTPError 401 when it begins observation; it never demonstrates its advertised streaming path.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:698-708`**

```text
fn minimal_decision_state() -> RuntimeDecisionState {
    RuntimeDecisionState::local_console(ConsolePolicy::default(), None)
}
```

A launch without auth_config uses the default require-app-auth policy and trusts no signing key. The explicit console_auth_required(False) override is not set in the example.

**`meerkat-mobkit/src/unified_runtime/http.rs:399-418`**

```text
Some(sse_decisions_a),
                access.clone(),
                Some(self.mob_runtime.clone()),
```

The bundled reference HTTP router passes the launch decision state to the agent SSE router, and likewise to the mob SSE router. These are not an unauthenticated exception to console policy.

**`meerkat-mobkit/src/http_sse.rs:256-262; 1095-1115`**

```text
let token = token.ok_or(())?;
    let auth =
        crate::runtime::resolve_authorized_console_auth_from_token(decisions, &token).ok_or(())?;
```

When require_app_auth is true, missing authentication is an error that the agent handler turns into HTTP 401. The false-policy branch is the only anonymous bypass.

**`sdk/python/meerkat_mobkit/runtime.py:3737-3768`**

```text
req = urllib_request.Request(url, method=method, data=body)
            req.add_header("Accept", "text/event-stream")
```

The Python SseBridge constructs the agent URL without an auth_token query and sends no Authorization header. Adding auth.jwt alone would not authenticate this particular SDK stream. The finding remains after applying the prior commit's ensure_member fix, so it is not a duplicate of that fix.

### Independent adjudication

Tried the potential exceptions: a loopback-only SSE bypass, a router constructed without decisions, SDK token forwarding, and automatic console opt-out. None applies to the bundled rpc_gateway path shown. The default policy requires authentication, the reference router explicitly supplies that policy to agent SSE, and the Python bridge supplies no bearer header or auth_token query parameter. Capturing an actual SseBridge.agent_events request with urlopen replaced by an in-memory response produced only Accept: text/event-stream. This is an independent downstream failure: the untouched quickstart may fail earlier because agent-1 does not yet exist, which is a separate prior-commit omission assigned to another agent. No live HTTP 401 reproduction is claimed.

**`docs/sdks/python.mdx:34-43`**

```text
    async with await MobKit.builder().mob("config/mob.toml").gateway("/path/to/rpc_gateway").build() as rt:
```

The builder has neither authentication nor the explicit local-console opt-out, then uses subscribe_agent.

**`meerkat-mobkit/src/decisions.rs:134-140`**

```text
            require_app_auth: true,
```

The policy default is protected, not implicitly open on loopback.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:706-708`**

```text
    RuntimeDecisionState::local_console(ConsolePolicy::default(), None)
```

minimal_decision_state supplies that default policy. Lines 13287-13306 select it when no auth configuration exists and pass the result to build_reference_app_router.

**`meerkat-mobkit/src/unified_runtime/http.rs:399-418`**

```text
                Some(sse_decisions_a),
```

The bundled reference router passes decisions to the agent SSE router, and Some(sse_decisions_b) to the mob SSE router. The decisions=None open-router exception does not apply.

**`meerkat-mobkit/src/http_sse.rs:1095-1115`**

```text
    let token = token.ok_or(())?;
```

Only require_app_auth=false bypasses the token requirement; otherwise absence fails. The agent handler at 256-262 maps that failure to sse_unauthorized, whose status is UNAUTHORIZED.

**`sdk/python/meerkat_mobkit/runtime.py:3746-3770`**

```text
            req.add_header("Accept", "text/event-stream")
```

agent_events constructs /agents/{id}/events without an auth query, and _stream_sse adds only Accept and optional Content-Type. The in-memory capture confirmed url=http://127.0.0.1:63210/agents/agent-1/events, headers={Accept: text/event-stream}.

**`sdk/python/meerkat_mobkit/runtime.py:637-640`**

```text
                self._config.console_require_app_auth
```

The public console_auth_required(False) builder option is actually serialized to the gateway; rpc_gateway.rs 6348-6353 applies it to the same decision state.

**Required correction:** Add .console_auth_required(False) to the loopback-only Python quickstart builder, with an explicit local-demo-only warning and a link to authenticated deployment guidance. Do not suggest that .auth(...) alone authenticates these headerless Python SseBridge subscriptions. Coordinate this with, but do not replace or duplicate, the separately audited member/configuration prerequisites.

### Changes and final verification

**Changed:** `docs/sdks/python.mdx`.

The quickstart explicitly sets console_auth_required(False), limits this to a fresh local loopback demo, links authentication/deployment guidance, and warns that Python SSE subscriptions do not forward bearer tokens and are not authenticated merely by configuring auth(...).

**Validation:** Executed the exact quickstart with the real builder and init serializer. Asserted runtime_options.console_require_app_auth is false and no non-default http_listen is supplied; captured the real headerless loopback SSE request. Confirmed the actual Rust SSE handler uses the decision-state auth gate.

**Final review: pass.** The quickstart opts out explicitly with console_auth_required(False), confines that choice to the default loopback listener in a fresh project, and warns that auth(...) does not add bearer tokens to the Python subscription requests. The real init serializer emitted console_require_app_auth=false; an actual SseBridge request captured in memory carried only Accept: text/event-stream. The Rust router and shared access function show that the opt-out controls this same SSE gate.

**`docs/sdks/python.mdx:50-56`**

```text
  This example disables console and SSE authentication for a local demo on
  the gateway's default loopback-only listener. Do not expose it to a shared
```

The new warning preserves the required local-demo boundary rather than normalizing unauthenticated remote exposure.

**`docs/sdks/python.mdx:63-70`**

```text
        .console_auth_required(False)
```

The executable builder chain contains the opt-out, not merely surrounding advice.

**`sdk/python/meerkat_mobkit/runtime.py:637-641`**

```text
            runtime_options["console_require_app_auth"] = (
                self._config.console_require_app_auth
            )
```

The real serializer transmits the setting. The independent quickstart probe asserted this actual init payload.

**`meerkat-mobkit/src/http_sse.rs:1095-1108`**

```text
    if !decisions.console.require_app_auth {
```

The false-policy branch permits anonymous access; the protected branch requires a token. The bundled reference router passes its decisions to agent SSE in unified_runtime/http.rs:399-418.

## D-003: send_and_wait is documented as request-specific despite a shared completion-cursor barrier

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/python.mdx:131-134`**

```text
# Delivery + wait for THAT turn's answer. These wait on the completion
# cursor the gateway captured before delivery, so they stay correct when an
# agent answers two turns identically.
answer = await alice.send_and_wait("Hello", timeout=90)
```

A host handling simultaneous conversations or connector traffic can attribute another turn's answer to this request while believing the API guarantees exact response correlation.

**`meerkat-mobkit/src/identity_first/runtime.rs:8061-8068`**

```text
// Read the completion baseline BEFORE delivery is attempted. The turn
        // this send starts can only complete after this point, so a caller
        // waiting past the baseline cannot miss it. The converse ambiguity is
        // deliberate and documented: on an identity receiving concurrent
        // traffic, another delivery's completion can also satisfy the wait.
        // Waiting too little beats the failure this replaces (waiting forever).
        let completion_baseline = self.rebase_completion_cursor(identity, token);
```

The runtime explicitly describes concurrent-delivery ambiguity. This is an intentional identity-wide barrier, not a per-request completion receipt.

**`sdk/python/meerkat_mobkit/runtime.py:1130-1132; 1193-1215`**

```text
progress = cursor.progress_since(baseline)
            if progress is CompletionProgress.COMPLETED:
                return inspection.output_preview
```

The wrapper returns whichever output_preview accompanies the first advanced cursor; it does not compare an interaction, message or turn ID. _wait_for_admission passes only completion_baseline into this wait.

**`sdk/python/meerkat_mobkit/identity_first_models.py:972-978`**

```text
if self.epoch != baseline.epoch:
            return CompletionProgress.INCARNATION_CHANGED
        if self.turns > baseline.turns:
            return CompletionProgress.COMPLETED
```

The check is only same epoch plus a greater count. A no-I/O reproduction using the real IdentityAgentHandle and a fake runtime returning baseline (7,1), observed (7,2), and another delivery's preview returned that other preview. The source-level runtime admission comment, not the fake alone, establishes that such concurrent completion is allowed.

### Independent adjudication

Attempted to find request/interaction correlation beyond the pre-send cursor and checked whether a gateway lock could eliminate the apparent ambiguity. The SDK wait consumes only an epoch/count baseline, and the Rust admission owner expressly permits another concurrent delivery's completion to satisfy it. Executing the real send_and_wait against a fake runtime with baseline (7,1), inspection cursor (7,2), and another delivery's preview returned that preview. This executable counterexample establishes the SDK's comparison rule; the Rust code establishes that such a concurrent observation is allowed. Consecutive identical answers are handled correctly, but THAT turn is an unjustified guarantee.

**`docs/sdks/python.mdx:131-134`**

```text
# Delivery + wait for THAT turn's answer. These wait on the completion
```

The defect is exact-request attribution, not the truthful repeated-identical-output benefit.

**`meerkat-mobkit/src/identity_first/runtime.rs:8061-8068`**

```text
        // deliberate and documented: on an identity receiving concurrent
        // traffic, another delivery's completion can also satisfy the wait.
```

The owning send path explicitly rules out interpreting this as a request-specific completion receipt.

**`sdk/python/meerkat_mobkit/runtime.py:1130-1132`**

```text
            progress = cursor.progress_since(baseline)
            if progress is CompletionProgress.COMPLETED:
                return inspection.output_preview
```

The public wait returns the preview associated with an advanced identity-wide count, not a matched request.

**`sdk/python/meerkat_mobkit/runtime.py:1193-1215`**

```text
        baseline = getattr(result, "completion_baseline", None)
```

_wait_for_admission threads only completion_baseline into wait_for_completion. It does not use any additional correlation field.

**`sdk/python/meerkat_mobkit/identity_first_models.py:971-978`**

```text
        if self.epoch != baseline.epoch:
            return CompletionProgress.INCARNATION_CHANGED
        if self.turns > baseline.turns:
            return CompletionProgress.COMPLETED
```

The cursor has no request identity. The in-memory reproduction used the actual class and waiter.

**Required correction:** Describe send_and_wait as waiting for an identity completion after the pre-delivery baseline, avoiding text comparison and handling repeated identical answers. Explicitly warn that concurrent/in-flight work can satisfy that barrier and the returned output_preview is not request-correlated. Exact-turn interpretation requires the caller to exclude other completion-producing work for that identity throughout the wait, including already-running kickoff or peer/background work.

### Changes and final verification

**Changed:** `docs/sdks/python.mdx`.

Replaced the exact-turn guarantee with an identity-wide completion barrier after the pre-delivery cursor baseline. Preserved repeated-identical-answer handling and explicitly states output_preview is not request-correlated; exact-turn interpretation requires excluding concurrent, already-running kickoff, peer, and background work throughout the wait.

**Validation:** Checked the owning send path at meerkat-mobkit/src/identity_first/runtime.rs:8061-8068, which expressly permits another delivery's completion to satisfy the baseline, and the adjudicated SDK waiter/CompletionCursor evidence. Reviewed adjacent example wording for an incompatible exact-request guarantee.

**Final review: pass.** The final text accurately describes an identity-wide completion barrier, preserves the identical-answer benefit, and expressly excludes request correlation. Its exact-answer interpretation is conditioned on excluding already-running kickoff, peer, and background work throughout the wait. Independently exercised the real send_and_wait with a pre-send baseline and a different delivery's later preview; it returned that preview as the corrected caveat predicts.

**`docs/sdks/python.mdx:186-190`**

```text
`send_and_wait` returns the `output_preview` observed after that identity's
completion cursor advances; the preview is not request-correlated.
```

The previous THAT-turn guarantee is gone, including in the adjacent code comment.

**`meerkat-mobkit/src/identity_first/runtime.rs:8061-8068`**

```text
        // deliberate and documented: on an identity receiving concurrent
        // traffic, another delivery's completion can also satisfy the wait.
```

The owner of the admission baseline explicitly permits the ambiguity.

**`sdk/python/meerkat_mobkit/runtime.py:1130-1132`**

```text
            progress = cursor.progress_since(baseline)
            if progress is CompletionProgress.COMPLETED:
                return inspection.output_preview
```

The waiter compares cursor progress and returns the observed preview, with no request-ID matching. The in-memory counterexample used this exact method.

## D-004: customize_build does not run on every identity reconcile

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/python.mdx:632-635`**

```text
On the identity plane the same hook is `AgentCustomizer.customize_build`,
registered with the public `.agent_customizer(customizer)`. It runs on fresh
create and on every restore/reconcile, receives the durable spec (so
`spec.profile` is the profile name), and registers tools on the draft:
```

Hosts may use reconcile to refresh prompts or replace tool handlers on already-active identities and silently keep the existing build because the promised hook does not run.

**`meerkat-mobkit/src/identity_first/runtime.rs:5260-5283`**

```text
if state == IdentityLifecycleState::Active {
            // Converged eager restore still validates the time-sensitive
            // external lease. This is part of the shared embodiment door, not
            // a second restore implementation: healthy authority is reused,
            // due authority is renewed, and lost authority parks this member.
            let record = self.reuse_active_restore_state(&spec).await?;
```

The Active branch returns an EmbodimentOutcome before the customizer invocation at line 5404. A healthy active identity is reused instead of rebuilt.

**`meerkat-mobkit/tests/identity_first_runtime.rs:9119-9170`**

```text
.expect("an already-active identity must bypass rebuild customization");
```

The existing regression test installs an always-failing customizer, reconciles an already-active identity and expects success, explicitly locking in the bypass. This test was read, not executed.

**`meerkat-mobkit/src/identity_first/orchestrator.rs:425-443`**

```text
/// This is the identity-first lazy bootstrap path: it validates the roster,
/// resolves cheap continuity metadata, records desired topology, and exposes
/// status/inspection surfaces. It deliberately does not acquire leases, call
/// customizers, load session snapshots, create sessions, or resume sessions.
```

Lazy roster registration is an additional concrete counterexample to 'every restore/reconcile'. Customization belongs to actual embodiment/build, not every metadata pass.

### Independent adjudication

Tried to distinguish actual cold restore from metadata reconciliation and to find a Python callback invocation outside Rust's embodiment path. The SDK reconcile RPC is gateway-owned; the current gateway preserves lazy bootstrap policy, and the shared embodiment path returns before customization when an active identity can be reused. The regression test deliberately supplies an always-failing customizer and expects active restore to succeed. Lazy roster registration also has no customization phase. Thus the stated universal frequency is false, while re-registering tools during a real cold build/resume remains valid.

**`docs/sdks/python.mdx:632-635`**

```text
create and on every restore/reconcile, receives the durable spec (so
```

Every reconcile is the overbroad condition to correct.

**`meerkat-mobkit/src/rpc.rs:4760-4796`**

```text
            let reconciled = match runtime.refresh_desired_topology().await {
```

The Python mobkit/reconcile_identity request uses the configured runtime bootstrap policy, with restore_flow_tracked as the unattached-context fallback, rather than independently invoking the customizer.

**`meerkat-mobkit/src/identity_first/runtime.rs:5260-5283`**

```text
            let record = self.reuse_active_restore_state(&spec).await?;
            self.clear_materialization_backoff(identity).await;
            return Ok(EmbodimentOutcome {
```

Healthy Active identities return here, before the actual customizer call at 5404.

**`meerkat-mobkit/src/identity_first/runtime.rs:5402-5404`**

```text
            let customize = customizer.customize_build(&build_context, &spec, &mut draft);
```

Customization is part of the materialization attempt reached only after the active-reuse branch.

**`meerkat-mobkit/tests/identity_first_runtime.rs:9119-9170`**

```text
    .expect("an already-active identity must bypass rebuild customization");
```

The test identity_first_runtime_restore_flow_reuses_exact_active_lease_without_customizing supplies FailingCustomizer and requires success. This regression-test source was inspected, not executed.

**`meerkat-mobkit/src/identity_first/orchestrator.rs:425-439`**

```text
/// status/inspection surfaces. It deliberately does not acquire leases, call
/// customizers, load session snapshots, create sessions, or resume sessions.
```

lazy_register_flow calls only register_roster_metadata, independently refuting the universal reconcile claim.

**Required correction:** Say customize_build runs during fresh creation and cold restore/resume materialization, not on every reconciliation. Explicitly state that an already-active converged identity is reused without customization, while lazy registration defers customization until materialization. Preserve the valid tool-registration guidance for actual rebuilds/resumes; do not claim reconcile refreshes active tool handlers.

### Changes and final verification

**Changed:** `docs/sdks/python.mdx`.

Scoped customize_build to actual fresh builds and cold restore/resume materialization. Explained active converged identity reuse and deferred lazy customization, and warned against using reconciliation to refresh already-active tool handlers.

**Validation:** Read the active-state early return in identity_first/runtime.rs:5260-5283 and the no-customizer lazy-register contract in identity_first/orchestrator.rs:425-443; both match the narrowed prose. Preserved the actual rebuild/resume tool-registration example.

**Final review: pass.** Customization is now tied to actual creation/cold materialization, not every reconcile. The active-converged reuse and lazy-registration exceptions are both explicit, and the page warns against using reconciliation to refresh active tool handlers. Independently followed the active early return, later customization call, lazy registration function, and existing failing-customizer regression.

**`docs/sdks/python.mdx:679-685`**

```text
cold restore/resume. An already-active converged identity is reused without
customization; lazy registration defers the hook until materialization.
Do not use reconciliation to refresh an already-active identity's tool
handlers.
```

Both adjudicated counterexamples and the practical consequence are preserved.

**`meerkat-mobkit/src/identity_first/runtime.rs:5260-5283`**

```text
            let record = self.reuse_active_restore_state(&spec).await?;
            self.clear_materialization_backoff(identity).await;
            return Ok(EmbodimentOutcome {
```

The active branch returns before customize_build at line 5404.

**`meerkat-mobkit/src/identity_first/orchestrator.rs:431-443`**

```text
/// status/inspection surfaces. It deliberately does not acquire leases, call
/// customizers, load session snapshots, create sessions, or resume sessions.
```

Lazy roster registration cannot satisfy a universal customization-on-reconcile promise.

**`meerkat-mobkit/tests/identity_first_runtime.rs:9119-9170`**

```text
    .expect("an already-active identity must bypass rebuild customization");
```

The test installs a failing customizer but requires active restore to succeed. The regression source was inspected, not executed.

## D-005: Both storage declaration examples build a disconnected SDK object rather than a gateway

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/python.mdx:485-498`**

```text
runtime = await (
    MobKit.builder()
    .mob("config/mob.toml")
    .persistent_state("state/mobkit")
    # Declare an in-memory runtime store (sessions do not survive restart).
    .runtime_store(runtime_store.memory())
    # Declare a bounded queryable in-process event log — or event_log.null().
    .event_log(event_log.memory(batch_size=64))
    .build()
)
```

Copying either storage example does not configure, open or validate the advertised stores; subsequent census or other RPC calls fail because no gateway was launched.

**`docs/sdks/typescript.mdx:266-277`**

```text
const runtime = await MobKit.builder()
  .mob("config/mob.toml")
  .persistentState("state/mobkit")
  // Declare an in-memory runtime store (sessions do not survive restart).
  .runtimeStore(runtimeStore.memory())
  // Declare a bounded queryable in-process event log — or eventLog.nullStore().
  .eventLog(eventLog.memory({ batchSize: 64 }))
  .build();
```

The TypeScript counterpart repeats the same complete builder chain with no gateway setter. These are new builders, not extensions of the earlier initialized runtime.

**`sdk/python/meerkat_mobkit/runtime.py:503-510; 584-591`**

```text
else:
            _log.warning(
                "MobKit runtime started without gateway or session builder — "
                "RPC calls will fail with NotConnectedError"
            )
```

Python starts a child and sends init only under if self._config.gateway_bin. There is no binary autodiscovery in this branch. Executing the documented chain with bytecode writes disabled yielded _transport=None, then status() raised NotConnectedError: runtime not started — no transport available; no state directory was created.

**`sdk/typescript/src/runtime.ts:649-653; 736-744`**

```text
console.warn(
        "[mobkit] runtime started without gateway or session builder — " +
          "RPC calls will fail with NotConnectedError",
      );
```

TypeScript likewise enters actual gateway initialization only when _config.gatewayBin is set, otherwise returns a disconnected runtime. No TypeScript execution is claimed; the control flow is explicit.

### Independent adjudication

Tried to find gateway autodiscovery, a previously configured builder, or local store opening performed by these SDK chains. Both snippets start a fresh MobKit.builder(), both default gateway fields are unset, and convention discovery fills configuration paths but not the executable. Executed the actual fenced Python builder expression and the actual TypeScript builder expression from the documents (providing their imported source symbols). Both returned a runtime without a transport, and the next mob-handle status call raised NotConnectedError: runtime not started — no transport available. No gateway, filesystem store, or external service was started. Although intentionally disconnected SDKs are supported for some host-only uses, that does not make these complete gateway storage examples operative.

**`docs/sdks/python.mdx:485-498`**

```text
    .event_log(event_log.memory(batch_size=64))
    .build()
```

The complete storage builder chain has no .gateway(...). MobKit is imported earlier on the page, so the missing local import is a standalone-snippet usability detail, not the central defect.

**`docs/sdks/typescript.mdx:266-277`**

```text
  .eventLog(eventLog.memory({ batchSize: 64 }))
  .build();
```

The TypeScript counterpart likewise creates a new builder but supplies no gateway executable.

**`sdk/python/meerkat_mobkit/runtime.py:503-510`**

```text
        if self._config.gateway_bin:
            transport = PersistentTransport(self._config.gateway_bin)
```

The transport and init are created only in the configured-gateway branch.

**`sdk/python/meerkat_mobkit/runtime.py:584-592`**

```text
                "MobKit runtime started without gateway or session builder — "
                "RPC calls will fail with NotConnectedError"
```

This is the branch reached by the exact documented Python expression; status() then raised the predicted typed exception.

**`sdk/typescript/src/runtime.ts:649-654`**

```text
    if (this._config.gatewayBin) {
```

TypeScript has the same transport prerequisite. The no-gateway branch at 736-744 only registers an optional session builder or warns.

**`sdk/typescript/src/builder.ts:834-864`**

```text
  private _applyConventionDefaults(): void {
```

Read the complete convention function: it only discovers console, access, gating and routing files, never rpc_gateway. Python builder.py 909-935 similarly has no executable discovery.

**Required correction:** Add .gateway("/path/to/rpc_gateway") to both complete storage declaration chains before .build(). For a self-contained Python block, include from meerkat_mobkit import MobKit alongside the existing config import. Preserve the explicit ephemeral runtime-store and bounded/null event-log semantics.

### Changes and final verification

**Changed:** `docs/sdks/python.mdx`, `docs/sdks/typescript.mdx`.

Both storage examples now configure gateway('/path/to/rpc_gateway') before build. The Python block imports MobKit locally. Explicit ephemeral runtime-store and bounded/null event-log semantics are unchanged.

**Validation:** Executed both exact storage builder expressions using real SDK source and mocked transport boundaries. Both instantiated a transport and sent mobkit/init containing runtime_store={storage:'memory'} and event_log={storage:'memory',batch_size:64}, rather than returning disconnected runtimes.

**Final review: pass.** Both complete storage builder chains now set the gateway executable, and the Python snippet imports MobKit locally. Independently executed each exact snippet with real SDK construction/serialization and mocked process transport only. Both allocated a transport and sent mobkit/init with runtime_store={storage:memory} and event_log={storage:memory,batch_size:64}. The explicit ephemeral and bounded/null-store caveats remain intact.

**`docs/sdks/python.mdx:529-543`**

```text
    .event_log(event_log.memory(batch_size=64))
    .gateway("/path/to/rpc_gateway")
    .build()
```

The Python example now enters the gateway branch rather than silently returning a disconnected runtime.

**`docs/sdks/typescript.mdx:268-279`**

```text
  .eventLog(eventLog.memory({ batchSize: 64 }))
  .gateway("/path/to/rpc_gateway")
  .build();
```

The TypeScript counterpart supplies the same prerequisite.

**`sdk/python/meerkat_mobkit/runtime.py:503-511`**

```text
        if self._config.gateway_bin:
            transport = PersistentTransport(self._config.gateway_bin)
            self._transport = transport
```

The executed SDK implementation allocates transport only with this configuration.

**`sdk/typescript/src/runtime.ts:649-654`**

```text
    if (this._config.gatewayBin) {
      this._transport = new PersistentTransport(this._config.gatewayBin, {
```

The actual TypeScript implementation has the same condition; the independent probe observed its init call.

## D-006: The TypeScript error-category enumeration omits the terminal actor-loop failure

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/typescript.mdx:315-322`**

```text
`category` is one of the `ErrorCategory` constants (`spawn_failure`,
`reconcile_incomplete`, `checkpoint_failure`,
`compaction_persistence_rejected`, `actor_loop_stalled`,
`actor_loop_recovered`, `host_loop_crash`, `rediscover_failure`,
`event_log_flush_failure`, `identity_materialization_failure`,
`mob_stop_proceeded_without_interrupt`), kept in lockstep with the Rust enum
by `tests/sdk_error_category_parity.rs`;
```

A host constructing exhaustive alert routing from the documented categories can miss the event that requires restarting a terminated mob actor, or mistake it for an unknown/recovery event.

**`sdk/typescript/src/types.ts:2169-2182`**

```text
ACTOR_LOOP_TERMINATED: "actor_loop_terminated",
```

The actual exported ErrorCategory contains this additional value. A mechanical set difference between its string constants and the documented list returned exactly ['actor_loop_terminated'].

**`meerkat-mobkit/src/unified_runtime/types.rs:619-637`**

```text
/// can restart the actor; the operator must restart the process. The
    /// delivery path fails fast on this state with `ActorTerminated`.
    ActorLoopTerminated {
```

This is a real terminal runtime event, not a future proposal or an alias for recovered. It is forwarded by the same gateway error hook as the listed events.

**`meerkat-mobkit/tests/sdk_error_category_parity.rs:29-32`**

```text
const RUST_ENUM_PATH: &str = "meerkat-mobkit/src/unified_runtime/types.rs";
const PYTHON_SDK_PATH: &str = "sdk/python/meerkat_mobkit/types.py";
const TYPESCRIPT_SDK_PATH: &str = "sdk/typescript/src/types.ts";
```

The cited parity test compares Rust with SDK source declarations, not this documentation list. Its existence does not keep the prose enumeration in parity.

### Independent adjudication

Tried treating the list as examples rather than exhaustive vocabulary and checked whether the missing tag was merely an alias or future feature. The text says category is one of the listed constants. The exported object actually has an additional terminal category, the Rust runtime emits it, and the gateway serializes every ErrorEvent without filtering it out. Loaded the actual TypeScript sources in memory and compared Object.values(ErrorCategory) against the documented list: the exact difference was [actor_loop_terminated]. parseErrorEvent preserves it and isResolutionErrorEvent returns false. The cited parity test covers SDK source constants, not this prose list.

**`docs/sdks/typescript.mdx:315-322`**

```text
`category` is one of the `ErrorCategory` constants (`spawn_failure`,
```

The closed list ends at mob_stop_proceeded_without_interrupt without actor_loop_terminated.

**`sdk/typescript/src/types.ts:2169-2182`**

```text
  ACTOR_LOOP_TERMINATED: "actor_loop_terminated",
```

This is an existing public constant, independently verified by executing the real source module.

**`meerkat-mobkit/src/unified_runtime/types.rs:619-637`**

```text
    /// can restart the actor; the operator must restart the process. The
    /// delivery path fails fast on this state with `ActorTerminated`.
    ActorLoopTerminated {
```

The terminal category has materially different operator consequences from recovery.

**`meerkat-mobkit/src/unified_runtime/mod.rs:2685-2695`**

```text
                ErrorEvent::ActorLoopTerminated {
```

The live actor-loop probe emits the variant through fire_error_hook; it is not just an unused enum member.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12591-12602`**

```text
                    b.notify("mobkit/on_error", params);
```

The generic forwarding hook serializes this event just like the other categories.

**`meerkat-mobkit/tests/sdk_error_category_parity.rs:29-32`**

```text
const TYPESCRIPT_SDK_PATH: &str = "sdk/typescript/src/types.ts";
```

The parity gate's inventory is SDK source, not docs/sdks/typescript.mdx.

**Required correction:** Add actor_loop_terminated to the category list. Briefly say it is a terminal actor failure requiring process restart, not actor_loop_recovered. Keep the source-constant parity guarantee without suggesting that its test validates the documentation enumeration.

### Changes and final verification

**Changed:** `docs/sdks/typescript.mdx`.

Added actor_loop_terminated to ErrorCategory enumeration, distinguished its process-restart requirement from actor_loop_recovered, and scoped the parity-test guarantee to SDK source constants rather than documentation.

**Validation:** Loaded real TypeScript source in memory and compared all 12 exported ErrorCategory values with the corrected prose enumeration: exact set equality. isResolutionErrorEvent is false for terminated and true for recovered. Rust ErrorEvent docs in unified_runtime/types.rs:619-637 independently confirm the restart requirement.

**Final review: pass.** The documented category set now exactly equals all 12 exported TypeScript ErrorCategory values. The new terminal/recovery distinction matches the Rust failure owner, and the parity-test guarantee is explicitly limited to SDK source constants. Independently loaded the SDK and asserted set equality plus terminated=false/recovered=true for isResolutionErrorEvent.

**`docs/sdks/typescript.mdx:319-328`**

```text
`actor_loop_recovered`, `actor_loop_terminated`, `host_loop_crash`, `rediscover_failure`,
```

The formerly missing category is present in the closed enumeration.

**`docs/sdks/typescript.mdx:323-328`**

```text
`actor_loop_terminated` is a terminal actor failure requiring a process restart,
not a recovery notification like `actor_loop_recovered`.
```

The operator-relevant distinction is explicit.

**`sdk/typescript/src/types.ts:2169-2182`**

```text
  ACTOR_LOOP_TERMINATED: "actor_loop_terminated",
```

This existing public constant was included in the executed set comparison.

**`meerkat-mobkit/src/unified_runtime/types.rs:619-630`**

```text
    /// can restart the actor; the operator must restart the process. The
    /// delivery path fails fast on this state with `ActorTerminated`.
```

The Rust contract independently establishes the process-restart requirement.

## D-007: The DurableAgentSpec reference incorrectly says all default-valued fields are omitted

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/python.mdx:152-154`**

```text
The roster provider returns one `DurableAgentSpec` per identity. The Rust side
deserializes the same shape, so every field below reaches the gateway
verbatim; fields left at their default are omitted from the wire.
```

The documented exact wire-shape rule does not reproduce the SDK's roster payload, leading to incorrect byte-level callback contract fixtures or forwarding implementations. This is a low-severity contract-description defect, not a claim that default fields themselves break the runtime.

**`sdk/python/meerkat_mobkit/identity_first_models.py:436-454`**

```text
result: dict[str, Any] = {
            "identity": self.identity,
            "profile": self.profile,
            "addressability": self.addressability,
        }
        if self.display_name is not None:
            result["display_name"] = self.display_name
        result["labels"] = dict(self.labels)
        if self.context is not None:
            result["context"] = self.context
        result["additional_instructions"] = list(self.additional_instructions)
```

addressability, labels and additional_instructions are always emitted, including their defaults. Direct execution of DurableAgentSpec('agent-1', 'worker').to_dict() returned {'identity': 'agent-1', 'profile': 'worker', 'addressability': 'addressable', 'labels': {}, 'additional_instructions': []}.

**`sdk/python/meerkat_mobkit/agent_builder.py:754-761`**

```text
specs = await provider.roster(context)
            return [s.to_dict() for s in specs]
```

The callback wire uses this serializer directly; there is no subsequent default-elision layer that would rescue the claim.

### Independent adjudication

Tried to find a later default-elision step in the roster callback so that the serializer alone might be misleading evidence. The callback returns each spec.to_dict() directly. Executed a real CallbackDispatcher roster callback with DurableAgentSpec('agent-1', 'worker'): the emitted payload included default addressability='addressable', labels={}, and additional_instructions=[]. The all-defaults-omitted claim is therefore false on the actual callback path. This is a low-severity wire-description issue; the defaults themselves are accepted and should not be changed.

**`docs/sdks/python.mdx:152-154`**

```text
verbatim; fields left at their default are omitted from the wire.
```

The broad omission claim contradicts the actual serialized defaults.

**`sdk/python/meerkat_mobkit/identity_first_models.py:436-454`**

```text
        result: dict[str, Any] = {
            "identity": self.identity,
            "profile": self.profile,
            "addressability": self.addressability,
        }
```

addressability is unconditional; labels and additional_instructions are also emitted unconditionally in the same serializer. Optional None fields are the omitted ones.

**`sdk/python/meerkat_mobkit/agent_builder.py:753-761`**

```text
            return [s.to_dict() for s in specs]
```

The actual callback returned [{'identity':'agent-1','profile':'worker','addressability':'addressable','labels':{},'additional_instructions':[]}]; no downstream callback default-elision layer exists.

**Required correction:** State that the SDK serializes the shared wire shape, always including identity, profile, addressability, labels and additional_instructions; optional None-valued fields are omitted. Keep the existing field table and the valid omission guarantees for unset runtime_mode_override and initial_message.

### Changes and final verification

**Changed:** `docs/sdks/python.mdx`.

Documented unconditional serialization of identity, profile, addressability, labels, and additional_instructions, with omission only for optional None-valued fields, explicitly including unset runtime_mode_override and initial_message.

**Validation:** Executed DurableAgentSpec('agent-1','worker').to_dict() and asserted the exact five-field default payload. Read the actual serializer and preserved the field table.

**Final review: pass.** The reference now distinguishes five unconditional fields from optional None-valued omissions. Independently evaluated the current serializer and obtained exactly identity, profile, addressability, labels, and additional_instructions for a default spec. The callback returns that serializer result directly, so no hypothetical later elision is needed.

**`docs/sdks/python.mdx:193-198`**

```text
`addressability`, `labels`, and `additional_instructions` are always included,
even at their defaults. Optional fields whose value is `None` are omitted,
```

The incorrect all-defaults-omitted rule has been replaced.

**`sdk/python/meerkat_mobkit/identity_first_models.py:436-454`**

```text
        result["labels"] = dict(self.labels)
        if self.context is not None:
            result["context"] = self.context
        result["additional_instructions"] = list(self.additional_instructions)
```

Default empty containers are unconditional; context is conditional. The runtime probe confirmed the full default payload.

**`sdk/python/meerkat_mobkit/agent_builder.py:753-761`**

```text
            return [s.to_dict() for s in specs]
```

The real roster callback forwards precisely the serialized spec.

## D-008: Both 0.6 changelogs attribute native roster reconcile reports to unrelated SDK module reconciliation

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`sdk/python/CHANGELOG.md:270-286`**

```text
- `mob_handle.reconcile(...)` may now return a report with a non-empty
  `failures` list (meerkat 0.6 collects per-identity failures rather than
  returning `Err` on the first failure). `UnifiedRuntime::reconcile()` in
  the Rust layer re-lifts this into an `Err` so callers using `?` see the
  same propagation behaviour they had pre-0.6; the Python SDK surfaces
  the full `failures` array in the response JSON when present.
```

The migration instructions tell Python/TypeScript users to inspect failures or report['spawned'] on an SDK result that never contained those fields, confusing module reconciliation with the native Rust roster operation.

**`sdk/typescript/CHANGELOG.md:254-264`**

```text
- Reconcile responses may now include a `failures` array when meerkat's
  native `MobHandle::reconcile` records per-identity failures. On the
  Rust side, mobkit re-lifts a non-empty `failures` list into an `Err`
  so callers using `?` see pre-0.6 propagation behaviour; the TS SDK
  surfaces the full array on the JSON response.
```

The TypeScript history repeats the same false SDK attribution and follows it with the native spawned-receipt shape.

**`sdk/python/meerkat_mobkit/runtime.py:533-536 in git commit 63f293c7b49af3cef07579e2a8ae0b53014603de`**

```text
async def reconcile(self, modules: list[str]) -> ReconcileResult:
        """Reconcile the mob to match the given module list."""
        raw = await self._runtime._rpc("mobkit/reconcile", {"modules": modules})
        return ReconcileResult.from_dict(raw)
```

Historical proof, obtained with git show 63f293c7:sdk/python/meerkat_mobkit/runtime.py. This is the very commit that introduced the quoted changelog paragraphs. Its ReconcileResult at types.py lines 82-93 has only accepted/reconciled_modules/added. The same method is present at v0.6.6 runtime.py lines 588-591 and current runtime.py lines 1345-1348.

**`sdk/typescript/src/types.ts:145-157 in git commit 63f293c7b49af3cef07579e2a8ae0b53014603de and tag v0.6.6`**

```text
export interface ReconcileResult {
  readonly accepted: boolean;
  readonly reconciledModules: readonly string[];
  readonly added: number;
}
```

Historical git-show reads also establish that parseReconcileResult constructs only these three fields; runtime.ts lines 422-425 in the introducing commit calls mobkit/reconcile with modules. There was no typed SDK failures/spawned return at the time being described.

**`meerkat-mobkit/src/rpc.rs:1065-1073 in git commit 63f293c7b49af3cef07579e2a8ae0b53014603de`**

```text
match runtime.reconcile_modules(modules.clone(), timeout).await {
                Ok(added) => JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: response_id,
                    result: Some(serde_json::json!({
                        "accepted": true,
                        "reconciled_modules": modules,
                        "added": added
                    })),
```

The wire endpoint itself returned module results, so an untyped client would not rescue the historical SDK claim either. In contrast, the same commit's unified_runtime/edge_reconcile.rs lines 59-68 really did introduce the native PartialFailure re-lift. This finding preserves that genuine historical change rather than rewriting history to today's implementation.

### Independent adjudication

Applied a historical rather than present-day comparison. The false Python/TypeScript SDK attribution was added in 63f293c7b49af3cef07579e2a8ae0b53014603de, whose SDK reconcile(modules) methods, parsers and actual mobkit/reconcile server handler all returned module results, not native roster reports. Rechecked the available released v0.6.6 commit dbfa66254780bf93133f77a852c7db481d753a63: the same SDK shapes and endpoint behavior persist. Separately validated the later receipt-shape paragraph against its introducing commit dd3ad6f912f8d04f91df07b6400a58b1b115ead9 and v0.6.6: the native Rust projection really did switch to MobSpawnReceiptWire, and native UnifiedRuntime::reconcile really did lift non-empty failures to PartialFailure. Those historical facts must remain. The correction is limited to confusing these native Rust operations with SDK MobHandle.reconcile(modules); it must not assert that every API named reconcile has the module result.

**`sdk/python/CHANGELOG.md:270-286`**

```text
  same propagation behaviour they had pre-0.6; the Python SDK surfaces
  the full `failures` array in the response JSON when present.
```

This historical SDK attribution, plus applying report['spawned'] to the SDK operation, is what requires correction.

**`sdk/typescript/CHANGELOG.md:254-264`**

```text
  so callers using `?` see pre-0.6 propagation behaviour; the TS SDK
  surfaces the full array on the JSON response.
```

The TypeScript changelog makes the same attribution.

**`sdk/python/meerkat_mobkit/runtime.py:533-536 at 63f293c7b49af3cef07579e2a8ae0b53014603de`**

```text
    async def reconcile(self, modules: list[str]) -> ReconcileResult:
        """Reconcile the mob to match the given module list."""
        raw = await self._runtime._rpc("mobkit/reconcile", {"modules": modules})
        return ReconcileResult.from_dict(raw)
```

git show of the exact commit that introduced the failures claim. The released v0.6.6 runtime has the same method at lines 588-591.

**`sdk/python/meerkat_mobkit/types.py:82-93 at 63f293c7b49af3cef07579e2a8ae0b53014603de`**

```text
class ReconcileResult:
    accepted: bool
    reconciled_modules: list[str]
    added: int
```

The parser immediately below constructs only these fields. At v0.6.6 the same definition/parser is at 103-114.

**`sdk/typescript/src/runtime.ts:422-425 at 63f293c7b49af3cef07579e2a8ae0b53014603de`**

```text
      await this._runtime._rpc("mobkit/reconcile", { modules }),
```

The contemporaneous TypeScript method also calls the module endpoint; v0.6.6 lines 500-504 are unchanged semantically.

**`sdk/typescript/src/types.ts:145-157 at 63f293c7b49af3cef07579e2a8ae0b53014603de`**

```text
    accepted: Boolean(d.accepted),
    reconciledModules: asStringArray(d.reconciled_modules),
    added: Number(d.added ?? 0),
```

The parser does not surface failures or spawned. Identical definition/parser lines exist at v0.6.6.

**`meerkat-mobkit/src/rpc.rs:1065-1073 at 63f293c7b49af3cef07579e2a8ae0b53014603de`**

```text
            match runtime.reconcile_modules(modules.clone(), timeout).await {
```

The success JSON contains only accepted, reconciled_modules and added. This rules out a raw/untyped client receiving native failures despite the SDK parser. At v0.6.6 the async endpoint has the same output at 1155-1163 and the synchronous endpoint at 304-312.

**`meerkat-mobkit/src/unified_runtime/edge_reconcile.rs:65-69 at dbfa66254780bf93133f77a852c7db481d753a63`**

```text
        if !report.mob.failures.is_empty() {
            return Err(UnifiedRuntimeReconcileError::PartialFailure(Box::new(
                report,
            )));
        }
```

Positive historical validation: native Rust PartialFailure behavior is real and must not be erased from the historical entry.

**`meerkat-mobkit/src/unified_runtime/types.rs:236-246 at dbfa66254780bf93133f77a852c7db481d753a63`**

```text
                MobSpawnReceiptWire {
                    member_ref: WireMemberRef::encode(mob_id, &identity_str),
                    agent_identity: identity_str,
                }
```

Positive historical validation: the spawned receipt shape is the native Rust wire projection. This was introduced separately from the failures paragraph, in dd3ad6f912f8d04f91df07b6400a58b1b115ead9.

**Required correction:** In both historical reconcile subsections, explicitly scope the failures, PartialFailure and spawned-receipt discussion to native Rust roster reconciliation and its wire projection. Replace the claims that the Python/TypeScript SDK MobHandle.reconcile surfaces those arrays with a historical clarification: reconcile(modules) is a different operation returning accepted/reconciled_modules/added in Python and accepted/reconciledModules/added in TypeScript. Do not present report['spawned'] as usage of that SDK result. Preserve the genuine historical Rust changes and all unrelated historical state-casing, storage and helper-session-id statements.

### Changes and final verification

**Changed:** `sdk/python/CHANGELOG.md`, `sdk/typescript/CHANGELOG.md`.

Scoped historical failures, PartialFailure, and spawned receipt shapes to native Rust roster reconciliation and its wire projection. Clarified that SDK MobHandle.reconcile(modules) returns accepted/reconciled_modules/added in Python and accepted/reconciledModules/added in TypeScript. Removed the misleading Python report['spawned'] usage. Kept the original heading/anchor and all unrelated historical facts.

**Validation:** Used git show on 63f293c7 for the historical Python reconcile method and TypeScript ReconcileResult/parser, and v0.6.6 for native PartialFailure behavior. A structural comparison against HEAD proved both changelogs are byte-identical outside their reconcile subsections.

**Final review: pass.** Both historical subsections now separate native Rust roster reports and their wire projection from the SDK module reconcile operation. They retain the genuine PartialFailure and spawned-receipt history and no longer tell SDK callers to read nonexistent failures/spawned fields. Independently read the claim-introducing 63f293c7 SDK methods/parsers/server response and the v0.6.6 native projection/lift. A byte comparison proved both changelogs are unchanged outside the reconcile subsection.

**`sdk/python/CHANGELOG.md:284-288`**

```text
These are native Rust roster changes, not Python SDK module-reconcile
results. The SDK's `MobHandle.reconcile(modules)` is a different operation:
it calls `mobkit/reconcile` and returns `ReconcileResult` with
`accepted`, `reconciled_modules`, and `added`, not `failures` or `spawned`.
```

The Python migration note now names the actual historical SDK result.

**`sdk/typescript/CHANGELOG.md:267-270`**

```text
These are native Rust roster changes, not TypeScript SDK module-reconcile
results. The SDK's `MobHandle.reconcile(modules)` is a different operation:
it calls `mobkit/reconcile` and returns `ReconcileResult` with
`accepted`, `reconciledModules`, and `added`, not `failures` or `spawned`.
```

The TypeScript note preserves camelCase SDK names rather than confusing them with raw JSON.

**`meerkat-mobkit/src/rpc.rs:2228-2230`**

```text
                        "accepted": true,
                        "reconciled_modules": modules,
                        "added": added
```

The current async module-reconcile handler emits only these module-result fields. The historical claim-introducing server was independently checked as recorded in historical_validation below; this citation now points exclusively to current source.

**`sdk/typescript/src/types.ts:315-315`**

```text
    reconciledModules: asStringArray(d.reconciled_modules),
```

The current TypeScript parser translates the module result to the documented camelCase field. Historical parser parity was separately checked as recorded below.

**`meerkat-mobkit/src/unified_runtime/edge_reconcile.rs:185-188`**

```text
        if !report.mob.failures.is_empty() {
            return Err(UnifiedRuntimeReconcileError::PartialFailure(Box::new(
                report,
            )));
```

The current native roster reconciliation still lifts nonempty failures to PartialFailure, distinct from the SDK module result. The historical v0.6.6 implementation was also checked.

**`meerkat-mobkit/src/unified_runtime/types.rs:345-347`**

```text
                MobSpawnReceiptWire {
                    member_ref: WireMemberRef::encode(mob_id, &identity_str),
                    agent_identity: identity_str,
```

The current native roster wire projection preserves the documented receipt fields. The historical v0.6.6 projection was separately verified; this literal excerpt uses current line numbers.

## D-009: The Python error-hook table incorrectly puts recovery notifications at ERROR level

**Severity:** low. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/python.mdx:410`**

```text
Without it the events reach only the gateway's stderr ERROR line
```

An operator relying on an ERROR-only log stream as the substitute for on_error will not see the recovery event promised by the table and may leave resolved incidents open.

**`meerkat-mobkit/src/unified_runtime/mod.rs:108-125`**

```text
if matches!(event, ErrorEvent::ActorLoopRecovered { .. }) {
        tracing::info!(
            error_event = ?event,
            hook_registered,
            "mobkit runtime error event resolved: {event}"
        );
    } else {
        tracing::error!(
```

The normal sink always distinguishes recovery (INFO) from failures (ERROR), independently of whether the Python host registered a callback. This is executable branching, not merely a competing comment.

**`docs/sdks/typescript.mdx:324-326`**

```text
The gateway's own stderr line for the same event
(ERROR, or INFO for `actor_loop_recovered`) is written either way
```

The sibling SDK page already describes the distinction accurately; Python should not promise the opposite logging level.

### Independent adjudication

Tried the alternative interpretation that ErrorEvent means only failures or that the SDK gateway has a different logging sink. ActorLoopRecovered is explicitly part of this ErrorEvent stream, the gateway forwarding hook does not filter it, and the normal Rust sink branches to INFO for it regardless of hook registration. The existing regression test explicitly requires INFO and forbids ERROR. Thus the Python blanket ERROR statement is inaccurate; the TypeScript page already preserves the distinction. Logging may additionally be suppressed by filters or stderr configuration, so the fix should not promise unconditional visible output.

**`docs/sdks/python.mdx:410`**

```text
Without it the events reach only the gateway's stderr ERROR line
```

The table claims the wrong severity for recovery notifications.

**`meerkat-mobkit/src/unified_runtime/mod.rs:108-125`**

```text
    if matches!(event, ErrorEvent::ActorLoopRecovered { .. }) {
        tracing::info!(
```

The else branch logs failures at ERROR. This branch is before the hook_registered check, so the severity distinction is independent of callback presence.

**`meerkat-mobkit/src/unified_runtime/mod.rs:3533-3553`**

```text
            !logged.contains("ERROR"),
            "a resolution must not log at ERROR: {logged}"
```

resolved_stall_is_not_logged_as_an_error also asserts INFO. The regression source was read, not executed.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:12591-12602`**

```text
                if let Ok(params) = serde_json::to_value(&event) {
```

The gateway forwards every serializable ErrorEvent as mobkit/on_error, including recovery.

**Required correction:** Replace the blanket stderr ERROR wording with the gateway's normal log sink: ERROR for failures, INFO for actor_loop_recovered, subject to configured tracing filters and stderr handling. Keep the existing sync/async and fire-and-forget callback semantics.

### Changes and final verification

**Changed:** `docs/sdks/python.mdx`.

The error-hook table now describes the normal gateway log sink: ERROR for failures and INFO for actor_loop_recovered, subject to tracing filters and stderr handling. Sync/async and fire-and-forget semantics remain unchanged.

**Validation:** Read log_error_event in meerkat-mobkit/src/unified_runtime/mod.rs:108-125 and verified the severity branch precedes callback-presence handling.

**Final review: pass.** The Python error-hook table now identifies ERROR failures, INFO recovery, and configured filtering/stderr handling. It preserves sync/async fire-and-forget callbacks without promising unconditional visible logging. The owning function performs this severity branch before checking whether a hook exists.

**`docs/sdks/python.mdx:454-454`**

```text
normal log sink: ERROR for failures, INFO for `actor_loop_recovered`, subject to configured tracing filters and stderr handling
```

The overly broad stderr ERROR guarantee has been replaced with the actual distinction and visibility caveats.

**`meerkat-mobkit/src/unified_runtime/mod.rs:108-132`**

```text
    if matches!(event, ErrorEvent::ActorLoopRecovered { .. }) {
        tracing::info!(
```

The recovery branch logs INFO; the adjacent else branch logs ERROR regardless of callback registration.

## D-010: The TypeScript raw memory configuration names a camelCase field rejected by gateway init

**Severity:** medium. **Decision:** confirmed. **Disposition:** fixed; independently reviewed.

### Original claim and proof

**`docs/sdks/typescript.mdx:248`**

```text
Gateway memory config accepts `{ backend: "local_json" }` with an optional `healthCheckEndpoint` (`memory.localJson(...)`)
```

A reader extending the displayed raw backend object with the documented optional field gets an initialization error instead of enabling the health check.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6736-6763`**

```text
let health_check_endpoint = match object.get("health_check_endpoint") {
```

The gateway reads snake_case only. The subsequent unsupported-key filter permits only backend and health_check_endpoint and returns 'unsupported runtime_options.memory_config fields: healthCheckEndpoint' for the camelCase key.

**`sdk/typescript/src/runtime.ts:314-324; 766-768`**

```text
const result: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
    result[k] = serializeConfig(v, seen);
  }
  return result;
```

The memory builder accepts a raw object and _buildInitParams passes it through serializeConfig. Ordinary object keys are preserved, not camel-to-snake translated, so passing {backend: 'local_json', healthCheckEndpoint: '...'} genuinely reaches the rejection path.

**`sdk/typescript/src/config/memory.ts:16-29`**

```text
const result: Record<string, unknown> = { backend: "local_json" };
      if (config.healthCheckEndpoint !== null) {
        result.health_check_endpoint = config.healthCheckEndpoint;
      }
```

The parenthesized helper is valid because its toDict method performs the translation. The defect is conflating that helper option with the accepted raw gateway config, not alleging the helper is broken.

### Independent adjudication

Narrow confirmation: memory.localJson({healthCheckEndpoint: ...}) is valid, so a claim that the TypeScript helper is broken would be rejected. The page instead attributes the camelCase field to the raw gateway object. Tried to find generic camel-to-snake conversion or a Rust alias; neither exists. Executed actual TypeScript builder/init serialization in memory: a raw object retained healthCheckEndpoint, the helper emitted health_check_endpoint, and the corrected raw object retained health_check_endpoint. The gateway's explicit key allowlist rejects the first object. No live gateway rejection was executed; the rejection is established by the exact parser branch.

**`docs/sdks/typescript.mdx:248`**

```text
Gateway memory config accepts `{ backend: "local_json" }` with an optional `healthCheckEndpoint` (`memory.localJson(...)`)
```

The sentence conflates a raw wire key with a helper option. It should explicitly distinguish the two accepted forms.

**`sdk/typescript/src/runtime.ts:314-325`**

```text
  for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
    result[k] = serializeConfig(v, seen);
  }
```

Plain-object keys are preserved. The actual _buildInitParams call at 766-768 uses this serializer for memory_config; in-memory execution confirmed the raw camelCase key survives unchanged.

**`sdk/typescript/src/config/memory.ts:16-29`**

```text
        result.health_check_endpoint = config.healthCheckEndpoint;
```

The helper is correct: its toDict conversion emits the snake_case wire field. In-memory execution confirmed this accepted form, not merely its interface declaration.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6736-6763`**

```text
                .filter(|key| key.as_str() != "backend" && key.as_str() != "health_check_endpoint")
```

The Rust parser admits only backend and health_check_endpoint, returning unsupported runtime_options.memory_config fields: healthCheckEndpoint for the documented raw extension. The value, if present, must also be a non-empty string.

**Required correction:** Separate the forms: the raw gateway object accepts {backend: "local_json", health_check_endpoint: url} with the health_check_endpoint key optional; the TypeScript helper is memory.localJson({healthCheckEndpoint: url}) and translates the key. Keep the legacy `elephant` backend deprecation and helper-only selector caveats unchanged.

### Changes and final verification

**Changed:** `docs/sdks/typescript.mdx`.

Separated raw gateway {backend:'local_json',health_check_endpoint:url} configuration from memory.localJson({healthCheckEndpoint:url}), stating that the endpoint is optional and the helper translates the key. Preserved `elephant` backend deprecation, helper selector, and auth caveats.

**Validation:** Evaluated both exact corrected memory expressions and memory.localJson() against the real TypeScript builder/init serializer in memory. Their payloads contain accepted snake_case keys, or omit the endpoint. Read the gateway's explicit health_check_endpoint allowlist at rpc_gateway.rs:6736-6763.

**Final review: pass.** The final sentence distinguishes the optional raw snake_case endpoint from the camelCase helper option, retaining deprecation and selector caveats. Independently passed both corrected forms and the no-endpoint helper through the actual TypeScript init serializer. The two configured forms emitted identical accepted snake_case keys, and the no-endpoint form omitted the field.

**`docs/sdks/typescript.mdx:250-250`**

```text
Raw gateway memory config accepts `{ backend: "local_json", health_check_endpoint: url }`, with `health_check_endpoint` optional. The TypeScript helper is `memory.localJson({ healthCheckEndpoint: url })`; it translates that option to the snake_case wire key.
```

The two distinct configuration contracts are no longer conflated.

**`sdk/typescript/src/config/memory.ts:16-29`**

```text
        result.health_check_endpoint = config.healthCheckEndpoint;
```

The helper performs the explicit translation.

**`meerkat-mobkit/src/bin/rpc_gateway.rs:6736-6763`**

```text
                .filter(|key| key.as_str() != "backend" && key.as_str() != "health_check_endpoint")
```

The gateway rejects other keys; ordinary raw TypeScript object serialization does not rename them.

## Independent scope checks

> [
>   "Read audit-brief.md, audit-scopes.json and every D-001 through D-010 finding; independently followed SDK, gateway and historical source rather than relying on the discovery verdicts.",
>   "Read-only git rev-parse confirmed HEAD af82b6b3ab34faed9bf3e962d148d55f10dcd1dc. git status --short was empty before and after validation.",
>   "PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=sdk/python python3 executed in-memory probes of real MobHandle subscription wrappers (three AttributeErrors, replacement fields valid), captured the actual unauthenticated SseBridge request, exercised real IdentityAgentHandle.send_and_wait with a distinct concurrent preview, evaluated the document's exact storage builder expression (NotConnectedError), and exercised a real roster CallbackDispatcher (default fields preserved). All assertions passed; no files/network/gateway/model execution.",
>   "NODE_DISABLE_COMPILE_CACHE=1 node --input-type=module used built-in registerHooks and stripTypeScriptTypes(mode='transform') to load the actual SDK TypeScript sources without writing compiled files or installing packages. Executed the exact storage builder expression (NotConnectedError), compared exported ErrorCategory values with the docs (only actor_loop_terminated missing), checked terminal resolution=false, and compared real init payloads for raw camelCase, helper and raw snake_case memory configs. All assertions passed. This was execution, not tsc typechecking.",
>   "The first Node invocation used an unsupported --disable-compile-cache CLI flag and exited before executing code. Retried successfully with NODE_DISABLE_COMPILE_CACHE=1; no installation or generated artifacts were needed.",
>   "git show verified the introducing failures commit 63f293c7b49af3cef07579e2a8ae0b53014603de, the separate spawned-receipt introduction dd3ad6f912f8d04f91df07b6400a58b1b115ead9, and released v0.6.6 at dbfa66254780bf93133f77a852c7db481d753a63, including both SDK methods/parsers, actual RPC success payloads, native PartialFailure and native receipt projections.",
>   "A read-only JSON/source validator checked complete one-to-one ID coverage, explicit verdicts, correction presence, and all 54 evidence quotes against their exact current or historical source line ranges."
> ]

## Final scope checks

> [
>   "Read audit-brief.md, audit-scopes.json, complete D audit/adjudication/fixes, assigned K-005/K-007/K-008/K-009 audit/adjudication, B-004/B-017 and E-009 audit/adjudication/fix entries, and fixes-coordination.json. fixes-K.json does not exist: D's assigned K fixes are recorded in fixes-D.json and the K-005 source-comment edits in fixes-coordination.json.",
>   "Read all five owned documents, all seven assigned-file diffs, and the actual SDK/gateway/lifecycle/HTTP/governance implementations. No repository edits, delegation, commits, package installs, or git-state mutations were performed.",
>   "PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=sdk/python python3 in-memory review harness passed: all 23 Python fenced blocks compile with top-level-await support; exact quickstart TOML parses; exact quickstart uses real builder/init/ensure/send/SSE logic with external process/file/HTTP boundaries mocked; observed ensure before send, console_require_app_auth=false, default loopback URL and headerless request. Exact observation loops print valid typed events.",
>   "The Python harness independently executed the exact storage snippet and asserted transport/init plus memory store/event-log wire values, exercised the real send_and_wait cursor ambiguity, checked exact default DurableAgentSpec wire fields, and tested RosterContext preservation for both empty and nonempty prior identities.",
>   "NODE_DISABLE_COMPILE_CACHE=1 node --input-type=module with built-in registerHooks/stripTypeScriptTypes loaded actual TypeScript source entirely in memory. Executed the exact storage snippet with mocked PersistentTransport process methods; asserted init storage declarations. Actual builder/init serialization passed for corrected raw memory, helper memory, and absent endpoint forms. All 12 ErrorCategory values exactly match the final list; terminal and recovery classifications passed. This is execution, not tsc typechecking.",
>   "Python AST comparison after removing docstrings matched HEAD exactly for identity_first_models.py. TypeScript byte comparison after removing block comments matched HEAD exactly for types.ts. The authorized source edits are documentary only; parsers still preserve caller-supplied prior-identity context.",
>   "git show independently verified historical SDK reconcile methods/parsers and actual server JSON at 63f293c7b49af3cef07579e2a8ae0b53014603de, plus native PartialFailure and MobSpawnReceiptWire projection at v0.6.6. Both changelogs are byte-identical to HEAD outside the approved reconcile subsection.",
>   "Read-only document checks passed for balanced fences and every absolute local SDK documentation page link target. Independently counted 63 unique current structural-event projection arms with no wildcard; the stale current-page count is gone and historical counts remain untouched.",
>   "git diff --check for all five owned docs and the two additionally authorized source-comment files passed.",
>   "Initial review resolved Cargo.lock and manifest pins to Meerkat 0.8.40 and independently disproved the former all-backends-disabled cache claim. That feedback is preserved in review_history; the final source-backed re-review confirms the corrected backend-dependent prose.",
>   "Rust contracts and existing regression-test sources were checked directly; no Rust build/test, credentialed model request, live gateway, or network call was run.",
>   "The initial report validator checked all 17 requested IDs and all 72 original evidence quotations against their then-current or historical line ranges. It caught one TypeScript changelog range starting one line late; that artifact-only range was corrected.",
>   "Residual re-review: read fixes-review-residuals.json and independently re-followed both SDK roster pages, all callback context sites, the exact provider backend selector, client construction, default fallback, unsupported-Automatic rejection, and Disabled-plus-TTL rejection. B-017 and R-D-001/R-D-002 are resolved; no new regression was found.",
>   "Residual re-review independently SHA-256-verified the existing local meerkat-anthropic-0.8.40.crate against Cargo.lock checksum cf5bf2e5b50fb570d70dd225d4192e762af04de8977449c5aea0110692f335cd and compared src/runtime/mod.rs and src/client.rs byte-for-byte with the archive, entirely in memory. No download, extraction, dependency installation, or generated source was needed.",
>   "Final docs checks: all 23 Python fenced blocks compile; all 10 TypeScript fenced blocks parse with built-in stripTypeScriptTypes (not tsc). Both changed SDK MDX pages compile with existing @mdx-js/mdx and remark-frontmatter dependencies. Both pages retain balanced fences and existing absolute local-link targets. Assigned-file git diff --check passes.",
>   "Refreshed all affected current TypeScript evidence ranges after the longer RosterContext comment, replaced B-017's old current-text citation, and added current citations for both K-005 SDK pages. Preserved original failure quotations solely in review_history rather than rewriting the historical feedback.",
>   "Final reviewed SDK page SHA-256: docs/sdks/python.mdx=bfe348c2eedb68381c71d88950ed30dc03a23dd70cf34bf034dcf6fe35d7eafb; docs/sdks/typescript.mdx=2b45c6694ba6f84b05f3890ee8fb9cdca0c4a5aeabdc66445d4e9b298f02eb76.",
>   "Final ledger validator passed: all 17 requested IDs have pass verdicts, all 69 current evidence quotations match their exact current/historical source ranges, zero regressions remain open, and review_history preserves the original failed item and both complete regression records with explicit resolved outcomes.",
>   "Proof-fidelity follow-up: D-008's four revision-qualified historical excerpts were replaced with exact current-source citation ranges (rpc.rs 2228-2230, TypeScript types.ts 315, edge_reconcile.rs 185-188, unified_runtime/types.rs 345-347). Historical verification remains explicitly recorded in D-008.historical_validation. All 69 active evidence excerpts were revalidated against current files without revision interpretation; pass verdicts and review_history are unchanged."
> ]

## Review feedback and resolution history

Earlier review failures are retained here; the per-item dispositions above reflect the final re-review rather than erasing the feedback.

```json
[
  {
    "review": "initial_wave4_before_residual_fixes",
    "previous_summary": {
      "items": 17,
      "pass": 16,
      "fail": 1,
      "regressions": 2,
      "ready_for_final_acceptance": false
    },
    "historical_evidence_note": "The following failure and regression evidence is preserved exactly as observed before the residual fixes. Old document quotations and ranges intentionally describe that review snapshot, not the final current text. Current proof is in items above.",
    "previous_failed_items": [
      {
        "id": "B-017",
        "verdict": "fail",
        "reason": "The repeated 0.8.32 version attribution was removed, but the replacement still asserts that the pinned dependency disables Anthropic caching on every backend. Independently inspected the actual Cargo.lock-resolved meerkat-anthropic 0.8.40 provider source, rather than trusting the old finding or coordination report. It defaults to Automatic for AnthropicApi, Vertex, and Foundry, and Disabled for Bedrock and Copilot. The client uses that default when no per-profile override exists and rejects explicit Automatic on unsupported backends. Thus the newly generalized pinned-dependency claim and universal opt-in framing remain materially wrong.",
        "evidence": [
          {
            "path": "docs/sdks/python.mdx",
            "lines": "715-731",
            "quote": "caching is such a knob; the pinned Meerkat dependency defaults it to\n`disabled` on every backend, so a long-lived identity opts in here:",
            "explanation": "Removing the stale version number did not make the retained backend-default assertion true."
          },
          {
            "path": "meerkat-mobkit/Cargo.toml",
            "lines": "59-64",
            "quote": "meerkat-client = { version = \"=0.8.40\" }",
            "explanation": "The current library consumes the 0.8.40 provider family, not the behavior described for old 0.8.32."
          },
          {
            "path": "Cargo.lock",
            "lines": "2251-2254",
            "quote": "name = \"meerkat-anthropic\"\nversion = \"0.8.40\"",
            "explanation": "The exact resolved Anthropic runtime version is 0.8.40; the meerkat-client lock entry depends on it."
          },
          {
            "path": "(operator-retained path)/meerkat-anthropic-0.8.40/src/runtime/mod.rs",
            "lines": "69-91",
            "quote": "fn default_cache_control_for_backend(backend: AnthropicBackendKind) -> AnthropicCacheControlPolicy {\n    if backend_supports_automatic_cache_control(backend) {\n        AnthropicCacheControlPolicy::Automatic\n    } else {\n        AnthropicCacheControlPolicy::Disabled\n    }\n}",
            "explanation": "This is the pinned registry source's operative default, not moving upstream documentation. Its adjacent exhaustive backend match returns true for AnthropicApi/Vertex/Foundry and false for Bedrock/Copilot."
          },
          {
            "path": "(operator-retained path)/meerkat-anthropic-0.8.40/src/client.rs",
            "lines": "859-884",
            "quote": "        let cache_control = anthropic_tag(request)\n            .and_then(|tag| tag.cache_control)\n            .unwrap_or(self.default_cache_control);",
            "explanation": "The actual request builder uses the backend default when no profile override is supplied. The adjacent branch rejects Automatic when automatic_cache_control_supported is false."
          }
        ],
        "required_correction": "Replace the blanket disabled-default claim with the current backend-specific policy: automatic on Anthropic API, Vertex, and Foundry; disabled on Bedrock and Copilot, where request-wide automatic is unsupported. Frame the existing example as explicitly selecting automatic caching and a one-hour TTL on a compatible backend, not as a universally necessary/supported opt-in. Preserve the valid nested provider_tag shape and the release-neutral wording."
      }
    ],
    "previous_k005_verdict_scope": "K-005 initially passed only its two explicitly assigned SDK source comments. Both copied SDK-page definitions still failed propagation and were recorded under R-D-001. The final pass now includes those pages.",
    "previous_regressions": [
      {
        "id": "R-D-001",
        "related_id": "K-005",
        "classification": "incomplete_fix",
        "severity": "medium",
        "title": "Both SDK reference pages retain the contradicted universal roster-context meaning",
        "reason": "The corrected source comments are accurate, but the final owned Python and TypeScript SDK pages still define previous_identities/previousIdentities as the identities registered at callback time, with emptiness explained only by bootstrap. This is the same already-adjudicated K-005 defect, not a new runtime or stylistic requirement. The unchanged copied definitions now directly contradict the corrected source comments. Providers following these pages can still mistake an empty full-refresh/reset context for an empty runtime.",
        "evidence": [
          {
            "path": "docs/sdks/python.mdx",
            "lines": "231-231",
            "quote": "Identities the identity runtime has registered at the time of the call. Empty on the bootstrap resolves (nothing is registered yet), populated on later re-derivations such as `mobkit/topology/query` or an edge reconcile.",
            "explanation": "The old universal meaning remains in the Python typed-context table."
          },
          {
            "path": "docs/sdks/typescript.mdx",
            "lines": "147-149",
            "quote": "   * Identities the identity runtime has registered at the time of the call.\n   * Empty on the bootstrap resolves, populated on later re-derivations such as\n   * `mobkit/topology/query` or an edge reconcile.",
            "explanation": "The TypeScript reference's copied interface comment still repeats it."
          },
          {
            "path": "meerkat-mobkit/src/identity_first/runtime.rs",
            "lines": "1280-1285",
            "quote": "            .roster(&RosterContext {\n                mob_definition: self.mob_definition.clone(),\n                previous_identities: Vec::new(),\n            })",
            "explanation": "A later full refresh passes no snapshot, independently proving the final SDK-page definitions are false."
          }
        ],
        "correction": "Propagate the already-correct Python/TypeScript source-comment qualification into docs/sdks/python.mdx's previous_identities row and docs/sdks/typescript.mdx's RosterContext snippet. Explicitly distinguish topology/edge snapshots from empty bootstrap/full-refresh/reset inputs and state that an empty list does not imply no identities are registered."
      },
      {
        "id": "R-D-002",
        "related_id": "B-017",
        "classification": "incorrect_retained_semantics",
        "severity": "medium",
        "title": "Release-neutral pin wording still promises the wrong Anthropic caching default",
        "reason": "This is the blocking final-semantics issue recorded under B-017, not an additional independent discovery count. Replacing '0.8.32 (the pinned release)' with 'the pinned Meerkat dependency' leaves and reasserts an all-backends-disabled claim contradicted by the pinned 0.8.40 implementation. It also presents explicit automatic caching as an unqualified opt-in, although Bedrock and Copilot reject that policy.",
        "evidence": [
          {
            "path": "docs/sdks/python.mdx",
            "lines": "718-727",
            "quote": "`disabled` on every backend, so a long-lived identity opts in here:",
            "explanation": "The problematic operational claim is retained adjacent to the coordinated pin edit."
          },
          {
            "path": "(operator-retained path)/meerkat-anthropic-0.8.40/src/runtime/mod.rs",
            "lines": "82-91",
            "quote": "        AnthropicBackendKind::AnthropicApi\n        | AnthropicBackendKind::Vertex\n        | AnthropicBackendKind::Foundry => true,",
            "explanation": "These backends support and default to Automatic; the same exhaustive match classifies Bedrock and Copilot as unsupported."
          },
          {
            "path": "(operator-retained path)/meerkat-anthropic-0.8.40/src/client.rs",
            "lines": "876-884",
            "quote": "        if matches!(cache_control, AnthropicCacheControlPolicy::Automatic)\n            && !self.automatic_cache_control_supported",
            "explanation": "An explicit automatic policy on unsupported backends fails request construction rather than enabling caching."
          }
        ],
        "correction": "Apply B-017.required_correction, then independently re-review the provider-parameters paragraph and example against the resolved provider implementation."
      }
    ],
    "residual_fix_report": "fixes-review-residuals.json",
    "resolutions": [
      {
        "ids": [
          "B-017",
          "R-D-002"
        ],
        "status": "resolved",
        "final_verdict": "pass",
        "reason": "Independently verified the replacement paragraph and example caveat against checksum-authenticated meerkat-anthropic 0.8.40 runtime/client source. Final prose distinguishes Automatic on Anthropic API/Vertex/Foundry from Disabled and unsupported Automatic on Bedrock/Copilot, and the disabled opt-out omits cache_ttl. Current exact evidence is retained under B-017."
      },
      {
        "ids": [
          "K-005",
          "R-D-001"
        ],
        "status": "resolved",
        "final_verdict": "pass",
        "reason": "Both SDK reference definitions now match the already-correct source comments and independently rechecked callback owners, including empty lists on full refresh/reset and the explicit warning against treating empty as an empty runtime. Current exact evidence is retained under K-005."
      }
    ],
    "final_re_review_summary": {
      "items": 17,
      "pass": 17,
      "fail": 0,
      "regressions": 0,
      "ready_for_final_acceptance": true
    }
  }
]
```
