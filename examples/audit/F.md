# Evidence group F

[Audit index](../AUDIT.md)

<a id="f01"></a>

### F01: Current peer_id-based send_message calls never produce phone arcs or routed log entries

**Severity:** high. **Verdict:** partial. **Examples:** 033.

**Original proof:** Run node ../../../../.copilot/session-state/eb71fac5-cb92-4b10-b39f-8d79f3799312/files/audit-F-probes.cjs from the worktree. The canonical send_message peer_id args probe sends a current-schema event, including display_name='the-office/finance/finance', and observes zero comms callbacks.

- [`examples/033-the-office-demo-sh/web/src/events.ts:9-22,98-118`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts) The event consumer reads args.to and splits a slash-delimited display name. A missing args.to yields no recipient, so neither startCall nor onMessage runs.
- [`meerkat-comms/src/mcp/tools.rs:62-83,282-303`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-comms/src/mcp/tools.rs) The current send_message input requires peer_id, body and handling_mode; display_name is optional diagnostics, not routing authority. There is no to field. peers explicitly says to discover the canonical peer_id.
- [`examples/033-the-office-demo-sh/web/src/agents.ts:21-39`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/agents.ts#L21-L39) Role prompts list display names as peers but do not teach canonical peer discovery.
- [`meerkat-web-runtime/src/lib.rs:2229-2247`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L2229-L2247) The runtime already exposes mob_member_peer_target for resolving member identities to canonical comms targets.

**Independent challenge:** The projection defect is independently confirmed: current peer_id-only send_message arguments produce no arc or routed log entry, while a legacy to argument exercises those branches successfully. Narrow the finding to the event consumer and its missing canonical identity mapping. The role prompt lists display names but does not explicitly instruct a to argument, and the actual peers/send_message tool descriptions already teach canonical peer discovery. Therefore an assertion that the prompt itself necessarily prevents real delivery is unsupported; actual delivery and this visualization failure are separate.

**Accepted correction:** Build a runtime-scoped peer_id-to-AgentId map from successful canonical member target resolution and project modern send_message requests through that map. Unknown peers must remain unknown, not be guessed from display labels. Clarifying the role prompt to use peers is reasonable adjacent documentation, not an independently established delivery fix. Projection of a requested call must not be described as proof of successful delivery.

- [`examples/033-the-office-demo-sh/web/src/events.ts:13-22,98-118`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts) The recipient is derived exclusively from args.to; the body, startCall and onMessage branches are conditional on that derived recipient.
- [`meerkat-comms/src/mcp/tools.rs:62-83,282-304`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-comms/src/mcp/tools.rs) SendMessageInput requires peer_id, body and handling_mode; display_name is diagnostics only. The peers description explicitly instructs discovery of canonical peer_id.
- [`examples/033-the-office-demo-sh/web/src/agents.ts:10-39`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/agents.ts#L10-L39) The skill names send_message and lists display labels. It does not specify the obsolete to field; this is counterevidence to treating prompting as a separately proven delivery defect.
- [`meerkat-web-runtime/src/lib.rs:2229-2278`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L2229-L2278) mob_member_peer_target resolves a live member into a canonical external peer descriptor. Independent real-WASM invocation returned external.peer_id for finance when its session remained available; a failed-start session correctly returned no_comms.

**Implementation (fixed):** Resolve every office member's canonical external peer_id before readiness and again after resume. Map send_message requests using only that runtime-scoped map; unknown peers are explicitly unknown. Reset mapping on teardown. Label arcs/logs as send requests, not confirmed delivery. Role prompts explicitly distinguish display names from peers-tool routing identities.
Changed: `examples/033-the-office-demo-sh/web/src/events.ts`, `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/src/types.ts`, `examples/033-the-office-demo-sh/web/src/agents.ts`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Actual drainAllEvents/resolution functions: peer_id-only requests create correctly attributed arcs; misleading display names cannot redirect; unknown peers are not guessed; reset drops old mappings; failed canonical resolution prevents readiness. Actual provider-backed comms delivery remains untested.

**Independent fix review (fixed):** The runtime-scoped map is built only from successful canonical member targets. peer_id, not optional display text or legacy to, selects the displayed recipient. Unknown IDs remain unknown, and projections are labelled send requests rather than delivery confirmations.
- [`examples/033-the-office-demo-sh/web/src/events.ts:12-34,92-104`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/events.ts) Canonical target resolution validates each peer, routing consults that map, and reset clears it.
- [`examples/033-the-office-demo-sh/web/src/main.ts:531-534,664`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts) The map is renewed during initial startup and actual resume.
- [`examples/033-the-office-demo-sh/web/src/agents.ts:12-16`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/agents.ts#L12-L16) Role instructions distinguish display names from canonical routing identity.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Canonical targets, misleading display labels, unknown recipients, restart reset and target-resolution failure passed. Reviewer also independently exercised the real event module using source.type=session envelopes and repeated provider IDs.

<a id="f02"></a>

### F02: Page-global tool-call deduplication drops subsequent Gemini actions

**Severity:** high. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** audit-F-probes.cjs supplies two different event envelopes from archivist and finance, both with legitimate provider tool ID fc_0. Only the first upsert callback executes.

- [`examples/033-the-office-demo-sh/web/src/events.ts:25-28,90-96`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts) A single never-reset Set stores payload.id across every member, turn and runtime restart. Duplicate IDs skip all host-side tool effects.
- [`meerkat-gemini/src/client.rs:2165-2167,2232-2241`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-gemini/src/client.rs) Every streamed provider request initializes tool_call_index=0 and creates fc_0, fc_1, etc.; IDs legitimately repeat across requests and members.
- [`meerkat-llm-core/src/adapter.rs:425-439`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-llm-core/src/adapter.rs#L425-L439) Provider-complete tool IDs are passed unchanged into block assembly.
- [`meerkat-core/src/agent/state.rs:6058-6069`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L6058-L6069) ToolCallRequested publishes tc.id unchanged.
- [`meerkat-core/src/event.rs:96-104`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs#L96-L104) The event envelope has an actual event_id, source and sequence for event identity.

**Independent challenge:** The page-global provider tool-call-ID cache incorrectly conflates independent events. This is not merely a hypothetical provider collision: the Gemini streaming implementation restarts its synthetic fc_N counter for each response. Same-member later requests are affected too, so adding member identity alone is insufficient. There is no need to claim that every provider repeats IDs or that every Gemini action is lost.

**Accepted correction:** Remove provider-call-ID deduplication. If replay protection is retained, key it by canonical envelope identity, bound it and reset it with the runtime. Do not scope by agent plus provider ID alone.

- [`examples/033-the-office-demo-sh/web/src/events.ts:25-28,90-96`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts) A never-cleared Set keyed only by payload.id skips the entire tool-call handling branch.
- [`meerkat-gemini/src/client.rs:2165-2167,2232-2241`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-gemini/src/client.rs) tool_call_index starts at zero for the stream; emitted IDs are fc_0, fc_1, etc.
- [`meerkat-llm-core/src/adapter.rs:425-438`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-llm-core/src/adapter.rs#L425-L438) The complete provider tool-call ID is forwarded to assembly without global namespacing.
- [`meerkat-core/src/agent/state.rs:6058-6068`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L6058-L6068) ToolCallRequested carries tc.id unchanged.
- [`meerkat-core/src/event.rs:93-104`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs#L93-L104) EventEnvelope owns independent event_id, source and seq identity.

**Implementation (fixed):** Remove provider-call-ID deduplication. Retain a bounded 4096-entry canonical envelope identity replay window, reset per runtime; request identity includes canonical source kind/session/event ID.
Changed: `examples/033-the-office-demo-sh/web/src/events.ts`, `examples/033-the-office-demo-sh/web/src/types.ts`, `examples/033-the-office-demo-sh/README.md`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Different envelopes using fc_0 across members and later requests from the same member all execute. Exact replay is suppressed. Runtime reset and bounded-window eviction are exercised by actual event draining.

**Independent fix review (fixed):** Provider call IDs and approval descriptions no longer control replay suppression. Canonical event_id plus member-session identity distinguishes independent response events, with a bounded 4096-entry window reset on runtime teardown. The unused source.kind lookup does not prevent source.type=session events from being processed or collapse independently UUID-identified events.
- [`examples/033-the-office-demo-sh/web/src/events.ts:7-16,81-88`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/events.ts) The guard keys on canonical envelope event_id/session identity and evicts its oldest entry above 4096; no provider tool-call ID appears in the key.
- [`meerkat-core/src/event.rs:91-105`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-core/src/event.rs#L91-L105) event_id is a canonical UUID, distinct from provider-local tool request IDs.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Same fc_0 across members/later requests is retained; true replay suppressed; reset and eviction tested. Independent reviewer canonical source.type probe retained all three same-fc_0 routing requests.

<a id="f03"></a>

### F03: Stale terminal-event fields hide authentication failures and structured results

**Severity:** high. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** audit-F-probes.cjs observes 'gate: Unknown error' for error_report={class:'llm',message:'Anthropic HTTP 401: invalid x-api-key'}. A run_completed with a natural-language result and valid structured_output, followed by extraction_succeeded, emits no headline.

- [`examples/033-the-office-demo-sh/web/src/events.ts:161-184,194-197`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts) run_failed reads payload.error rather than error_report.message; run_completed parses only result as JSON and is conditional on nonempty result; extraction_succeeded is ignored. The tool-start branch also uses obsolete tool_call_start.
- [`sdks/web/src/generated/events.ts:795-824,914-922`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/sdks/web/src/generated/events.ts) Current events provide RunCompleted.structured_output, ExtractionSucceeded.structured_output, RunFailed.error_report, and tool_execution_started.
- [`meerkat-core/src/agent/runner.rs:1517-1555`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/runner.rs#L1517-L1555) The runtime separately emits result.text and structured_output, and can emit an extraction_succeeded event.
- [`examples/033-the-office-demo-sh/web/src/main.ts:604-637`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts#L604-L637) Automatic key-recovery UI depends on the actual error message reaching the consumer's errors array.

**Independent challenge:** All cited current-contract mismatches are present. Authentication diagnostics become Unknown error; current structured_output/extraction_succeeded are ignored; empty result prevents terminal visual cleanup; tool_execution_started is ignored. Counterevidence limits the claim: a run_completed whose result already contains a JSON headline still works, so this is not proof that all structured summaries are universally absent. Real paused-WASM subscriptions did not deliver a terminal event during the bounded probe; the terminal-field reproduction is a canonical-event DOM fixture backed by Rust/generated definitions, not a claimed live-provider test.

**Accepted correction:** Consume the current typed event discriminants and error_report.message; always perform terminal cleanup regardless of text; use structured_output/extraction_succeeded according to the extraction lifecycle while avoiding duplicate summaries. Preserve working JSON-text fallback only if intentionally supported.

- [`examples/033-the-office-demo-sh/web/src/events.ts:161-197`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts#L161-L197) Completion is gated on truthy result and JSON.parse(result); failure reads payload.error; tool-start discrimination uses tool_call_start.
- [`sdks/web/src/generated/events.ts:795-824,914-918`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/sdks/web/src/generated/events.ts) Current fields are RunCompleted.structured_output, ExtractionSucceeded.structured_output, RunFailed.error_report and tool_execution_started.
- [`meerkat-core/src/agent/runner.rs:1517-1553`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/runner.rs#L1517-L1553) The implementation emits text and structured_output separately and exposes the extraction-succeeded event.
- [`examples/033-the-office-demo-sh/web/src/main.ts:602-637`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts#L602-L637) Key recovery searches the errors array for actual auth/key diagnostics; Unknown error cannot match.

**Implementation (fixed):** Read error_report.message and tool_execution_started. Always clear terminal agent visuals, including empty result. Consume structured_output and the run_completed/extraction_required/extraction_succeeded lifecycle exactly once, retaining deliberate JSON-text fallback. Authentication recovery actually stops agents before showing configuration when possible.
Changed: `examples/033-the-office-demo-sh/web/src/events.ts`, `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Current event fixtures execute actual projection: empty and natural-language result plus structured output, deferred extraction and duplicate extraction terminal, legacy JSON result, current tool execution event, all terminal idle cleanup. Actual polling interval routes a typed authentication failure into the visible key dialog and calls stop.

**Independent fix review (fixed):** Current failure and tool-start fields are read; terminal visuals always clear even for empty results. Structured output follows main completion/extraction terminal state, suppressing duplicate extraction summaries. Authentication failures invoke real lifecycle pause before configuration when possible.
- [`examples/033-the-office-demo-sh/web/src/events.ts:58-71,112-142`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/events.ts) Terminal cleanup is separate from output text; typed carriers/error_report and current tool_execution_started are used.
- [`examples/033-the-office-demo-sh/web/src/main.ts:681-713`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts#L681-L713) The polling diagnostic path awaits pauseOffice and exposes actionable key recovery.
- [`meerkat-core/src/event.rs:2055-2097`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-core/src/event.rs#L2055-L2097) Canonical terminal event fields corroborate the corrected consumer.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Current terminal/extraction fixtures, empty result cleanup, retained intentional JSON-text fallback, duplicate extraction and actual polling/auth dialog path passed.

<a id="f04"></a>

### F04: Pause hides event processing but leaves autonomous agents running

**Severity:** high. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** The pause probe starts a mocked runtime, clicks Pause, observes PAUSED with zero runtime lifecycle calls, then injects Expense Report and observes mob_member_send while polling remains disabled.

- [`examples/033-the-office-demo-sh/web/src/main.ts:600-603,659-680,693-715,782-794`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) Pause only flips the local running flag checked by the polling interval. It invokes no runtime lifecycle operation. Scenario and chat sends check only runtime and mobId, so they remain accepted while the status says Office paused.
- [`examples/033-the-office-demo-sh/web/src/agents.ts:230-241`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/agents.ts#L230-L241) All members are autonomous_host agents, not agents whose execution is driven by the UI poll.
- [`meerkat-web-runtime/src/lib.rs:2885-2897,2918-2934`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs) Member subscriptions are draining broadcast receivers and explicitly produce StreamTruncated when the receiver lags.

**Independent challenge:** Pause is exclusively a UI polling gate, not runtime control, and new scenario/chat work remains admissible. The independent mock test observes no lifecycle calls; a real-WASM member accepted a queued delivery receipt while running=false. Buffer-loss risk follows the draining broadcast contract but was not induced as a stress test. The narrowed real runtime probe stubbed topology wiring after a separately bounded full-topology timeout; it must not be presented as a complete office end-to-end success or proof of model spending.

**Accepted correction:** Make the chosen contract truthful. For execution pause, await mob_lifecycle stop/resume, gate new work during stopping/stopped states, report errors instead of optimistic badges, and re-establish subscriptions if lifecycle semantics require it. Alternatively explicitly label a visualization-only freeze and continue draining/performing host tool effects while separating visual updates. Do not invent an unsupported mob_pause export.

- [`examples/033-the-office-demo-sh/web/src/main.ts:600-603,659-680,693-715,782-794`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) Pause only toggles running, which prevents drainAllEvents. Scenario and chat admission check runtime/mobId but not running.
- [`examples/033-the-office-demo-sh/web/src/agents.ts:230-241`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/agents.ts#L230-L241) Profiles use autonomous_host; execution is not driven by the UI's polling timer.
- [`meerkat-web-runtime/src/lib.rs:2013-2037,2886-2897,2912-2933`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs) The actual lifecycle entry point is mob_lifecycle. Member subscriptions drain buffered events and emit StreamTruncated on lag; pausing drain is not pausing execution.
- [`meerkat-mob/src/runtime/handle.rs:11527-11566`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/handle.rs#L11527-L11566) Supported stop/resume transitions are machine-owned; stop documents rejection of mutation commands while stopped and surfaces cleanup/interrupt failure.

**Implementation (fixed):** Retain execution pause: Pause agents awaits mob_lifecycle stop, Resume agents awaits resume and renews peer targets/subscriptions. Stop/resume transitions block scenario/chat/approval admissions. Errors are not optimistic STOPPED/LIVE badges; recovery requires Restart. Polling continues to drain host effects and explicitly reports stream lag.
Changed: `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/src/events.ts`, `examples/033-the-office-demo-sh/web/src/types.ts`, `examples/033-the-office-demo-sh/README.md`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`, `examples/033-the-office-demo-sh/web/tests/provider-fixture.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Actual pause DOM handler and functions: deferred stop, concurrent input rejection, stopped/resumed state, subscription renewal, failed stop, continued archive effects and visible lag warning.
- `cd examples/033-the-office-demo-sh/web && npm run test:offline`: **fail** Real WASM isolated one-member lifecycle passes. Valid bounded synthetic Anthropic responses now exercise the actual HTTP/provider boundary: 21 successful requests, ten healthy idle members and ten successful real subscriptions while the first wire remains pending. It still rejects after 30 seconds; office-wide pause/resume remains blocked. See fix-F-runtime-repro.json for source/typed-state evidence identifying the wasm32 no-op comms-drain readiness branch.

**Independent fix review (fixed):** Pause requests and awaits actual mob stop, gates new UI work while stopping/stopped, and resumes with fresh peer targets/subscriptions. Polling continues host effects and reports lag. Failure is ERROR/Restart, not optimistic STOPPED/LIVE. Two final ordinary real-WASM Office runs now verify the complete stop/admission/resume path with zero fixture errors.
- [`examples/033-the-office-demo-sh/web/src/main.ts:455-465,519-547,739-741,772-774,956-960`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts) Controls and handlers gate admissions; stop/resume await acknowledged lifecycle and recreate subscription handles.
- [`examples/033-the-office-demo-sh/web/src/main.ts:676-719`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts#L676-L719) Polling has no running-only early return, so pause does not silently suppress host effects.
- [`examples/033-the-office-demo-sh/web/src/events.ts:140-146`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/events.ts#L140-L146) Lost events are explicitly surfaced as unrecoverable host-effect risk.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Admission barriers, awaited badges, renewed subscriptions, lag and failed-stop behavior passed.
- `npm --prefix examples/033-the-office-demo-sh/web run test:offline`: **pass** Two independent final ordinary runs pass real ten-member stop, rejected stopped admission, resume with ten renewed subscriptions, and destroy. All requests classified/reconciled; zero errors and pending fixture responses.

<a id="f05"></a>

### F05: Canceling the first API-key prompt removes every visible way to start

**Severity:** medium. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** audit-F-browser.cjs uses real Chrome: load without keys, click Start, Cancel the API-key prompt, then configure a synthetic key through Settings and close it. There are zero visible buttons whose labels contain Start. The status still says start is blocked; Pause is inert. The script separately verifies startup by programmatically clicking the hidden button, explicitly not treating that as user recovery.

- [`examples/033-the-office-demo-sh/web/src/main.ts:751-755,759-763,918-932,648-651`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) The initial Start click immediately hides its overlay. Cancel hides the key dialog without restoring Start. Settings has only Close; it cannot start/retry. The startup catch likewise leaves the Start overlay hidden.
- [`examples/033-the-office-demo-sh/web/src/styles.css:445-452`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/styles.css#L445-L452) The splash is a full-screen fixed overlay above the top bar; the initial gear instructions cannot be followed by pointer before dismissing it.

**Independent challenge:** The cancellation/failure dead end is real in the actual DOM. The hidden start button can be invoked programmatically, but that is not an ordinary visible recovery path. The gear can configure a key after cancellation but closing Settings does not initiate startup. No persistent Restart implementation is independently required beyond restoring a visible, reliable Start/Retry path for these failures.

**Accepted correction:** Restore a visible Start/Retry control on cancellation and startup failure, make initial setup reachable, and prevent concurrent startup operations. If adding Restart, ensure it actually performs runtime teardown/reinitialization rather than merely changing stored form values.

- [`examples/033-the-office-demo-sh/web/src/main.ts:648-651,751-763,918-932`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) Start hides its overlay; cancellation and the startup catch never restore it; Settings only opens/closes.
- [`examples/033-the-office-demo-sh/web/src/styles.css:434-440`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/styles.css#L434-L440) The splash covers the viewport; hidden state disables the splash controls. The initial gear-first instructions are behind the splash.

**Implementation (fixed):** Add visible top-bar Start/Retry/Restart control, clarify initial PRESS START key setup, prevent overlapping startup/lifecycle operations, and implement actual mob/runtime teardown before restart. Cancelled key setup and rejected initialization retain visible recovery.
Changed: `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/README.md`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Visible DOM controls cancel initial setup, open Settings/configure keys, fail initialization, retry successfully, and ignore a second click while spawn is pending. No hidden Start invocation.
- `cd examples/033-the-office-demo-sh/web && npm run test:offline`: **fail** Strict healthy-bootstrap assertion fails on upstream wire timeout; before failing, the real built page verifies ERROR, destroyed/absent runtime, and visible enabled Retry.

**Independent fix review (fixed):** Cancel/failure leaves a visible Start/Retry control. Startup is excluded while initialization or lifecycle work is active. Restart performs actual subscription/mob/runtime teardown, not just a settings update.
- [`examples/033-the-office-demo-sh/web/src/main.ts:455-502,550-574,722-732,832-863`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts) Visible control recovery, lifecycle exclusion and explicit teardown are wired into actual startup handlers.
- [`examples/033-the-office-demo-sh/README.md:72-76`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/README.md#L72-L76) Instructions describe reachable first-use, cancellation and retry paths.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Actual DOM cancel/setup/rejected-init/retry and double-start tests passed. Failed partial bootstrap cleanup also passed in six independent failure configurations.

<a id="f06"></a>

### F06: Approval detail renders model-controlled text as active HTML

**Severity:** high. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** The harmless-markup probe sends action_description='<em data-audit="only-markup">Synthetic</em>' and proposed_by='<b>Synthetic proposer</b>', expands the item, and confirms both tags occur verbatim in approvalDetailBody.innerHTML. No exfiltration or active payload was used.

- [`examples/033-the-office-demo-sh/web/src/events.ts:121-131`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts#L121-L131) Approval strings originate in agent tool-call arguments.
- [`examples/033-the-office-demo-sh/web/src/main.ts:847-866`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts#L847-L866) The compact summary is escaped, but expanded action_description and proposed_by are interpolated verbatim into innerHTML. risk_level is also interpolated into markup.

**Independent challenge:** Provider-controlled description and proposer strings become actual elements in the expanded approval DOM. The compact list correctly escapes its summary, which limits the affected sink. This adjudication used harmless markup only; it did not execute an active payload or demonstrate data exfiltration. The risk schema already restricts valid inputs, so arbitrary risk-class injection is not needed to establish the confirmed unescaped text defect.

**Accepted correction:** Render approval text using textContent/text nodes or an equivalent correctly escaped construction. Map risk presentation to fixed allowed values/classes and preserve ordinary risk styling. No active-HTML capability is needed for this tool.

- [`examples/033-the-office-demo-sh/web/src/events.ts:121-131`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts#L121-L131) Description and proposer are copied from the agent's request_human_approval arguments.
- [`examples/033-the-office-demo-sh/web/src/main.ts:506-518,847-866,894-896`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) The tool schema permits arbitrary description/proposer strings. The expanded detail interpolates them into innerHTML, unlike the escaped compact summary.
- [`examples/033-the-office-demo-sh/README.md:78-80`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/README.md#L78-L80) The documented non-authoritative role-play boundary does not sanitize HTML; this disclaimer is not counterevidence to a DOM injection defect.

**Implementation (fixed):** Escape description/proposer in expanded approval rendering and map risk to fixed low/medium/high/unknown presentation classes. Preserve compact escaping and working approval buttons.
Changed: `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Actual compact/expanded DOM renders harmless HTML-looking descriptions/proposers literally with no injected descendants. Low, medium and high classes render correctly, and each actual Deny button handler emits the correct decision.

**Independent fix review (fixed):** Expanded description and proposer are escaped text in element-body contexts. Risk is selected from fixed permitted presentation values, with unknown fallback; compact text remains escaped and normal decision controls work.
- [`examples/033-the-office-demo-sh/web/src/main.ts:930-942,947-950,1004-1006`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts) Model text is escaped before HTML insertion; only a fixed risk value can enter the class name.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Harmless markup remains literal in compact and expanded views; no injected element appears; low/medium/high styling and approve/deny paths pass.

<a id="f07"></a>

### F07: Description-prefix deduplication permanently suppresses distinct approval requests

**Severity:** medium. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** The approval probe submits four unique call IDs: two legitimate repeat descriptions and two different descriptions sharing a 60-character prefix. Only two callbacks occur, despite four distinct requests.

- [`examples/033-the-office-demo-sh/web/src/events.ts:27-28,48-53`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts) Every approval is keyed by the first 60 lowercased description characters in a page-lifetime Set; entries are never released after a decision or runtime restart.

**Independent challenge:** The second independent deduplication layer suppresses requests even after bypassing provider-ID collisions with unique call IDs. Description equality or a shared prefix is not request identity. The set is never retired, so resolving the UI item cannot permit a later new request with the same description.

**Accepted correction:** Remove description-prefix identity. Carry canonical event/request identity through approval creation and deduplicate only true replays, sharing the correct event-identity mechanism with F02 where appropriate.

- [`examples/033-the-office-demo-sh/web/src/events.ts:27-28,48-53,121-131`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts) The lifetime set uses only the first 60 lowercased description characters; no event/request identity or completed-state release is involved.
- [`examples/033-the-office-demo-sh/web/src/main.ts:307-317,871-891`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) UI IDs are assigned only after fireApproval admits the request; resolveApproval does not clear the module's description set.

**Implementation (fixed):** Remove description-prefix approval identity. Carry canonical envelope request identity into pending approvals, relying only on bounded true-envelope replay protection.
Changed: `examples/033-the-office-demo-sh/web/src/events.ts`, `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Distinct events with identical descriptions, matching 65-character prefixes and case variations all produce independent items; exact event replay produces one. A fresh same-wording request after resolution appears again.

**Independent fix review (fixed):** No description or description-prefix identity remains. New approval requests carry their canonical event-derived identity; repeated wording and shared prefixes remain independent, while only true envelope replay is suppressed.
- [`examples/033-the-office-demo-sh/web/src/events.ts:81-88,105-106`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/events.ts) Canonical request identity is forwarded through the shared envelope guard.
- [`examples/033-the-office-demo-sh/web/src/main.ts:319-331`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts#L319-L331) Every admitted request becomes a distinct pending item retaining request_id and runtime epoch.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Identical descriptions, common 65-character prefixes, differing case, true replay and new same-worded requests after resolution passed.

<a id="f08"></a>

### F08: Approval decisions disappear before successful delivery to Gate

**Severity:** medium. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** The source probe substitutes an observable thenable for mob_member_send. The item is immediately removed and neither then nor catch is used. A rejected real promise therefore has no recovery path and leaves no retryable item.

- [`examples/033-the-office-demo-sh/web/src/main.ts:871-891`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts#L871-L891) resolveApproval splices the pending item first, calls the async mob_member_send without awaiting or catching it, and immediately logs/displays the human decision as if delivered.

**Independent challenge:** A rejected delivery removes the only actionable approval and produces an unhandled rejection. The log can truthfully say the human chose Approved before delivery, so that phrase alone is not proof of delivery misrepresentation; the confirmed defect is irreversible UI removal with no delivery/error/retry state. Introducing a new cross-agent approval protocol is not required merely to fix this rejected-send path.

**Accepted correction:** Track decision/sending/failure state, await the send receipt, retain a retryable item on failure and prevent duplicate clicks. Only label delivery accepted after the receipt. Preserve enough of the request identity/content to retry the same decision; any broader correlation protocol should be designed alongside F07 rather than assumed proven here.

- [`examples/033-the-office-demo-sh/web/src/main.ts:871-891`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts#L871-L891) The item is spliced before mob_member_send, whose promise is neither awaited nor caught. The panel is immediately re-rendered without it.
- [`meerkat-web-runtime/src/lib.rs:2464-2502`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L2464-L2502) mob_member_send is asynchronous and returns a delivery receipt only after the canonical member send succeeds; failures reject.

**Implementation (fixed):** Await and validate Gate inbox receipt before removing approval. Track pending/sending/failed/expired, retain decision and request identity across retry, prevent duplicate clicks and contradictory decisions while retrying, and expire old-runtime requests across teardown. Catch rejected sends and display actionable retry errors.
Changed: `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`, `examples/033-the-office-demo-sh/README.md`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Actual DOM click rejects once, retains visible error and same decision, retries to one accepted receipt, prevents double-click concurrent send, retains a missing-runtime error, expires requests on restart, and ignores a late old-runtime receipt. No unhandled promise rejections.

**Independent fix review (fixed):** The item remains until a validated Gate inbox receipt succeeds. Decision/sending/failure state suppresses duplicate and contradictory clicks, preserves the same request on retry and expires it across teardown. A receipt is described as inbox acceptance, not execution.
- [`examples/033-the-office-demo-sh/web/src/main.ts:956-1001`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts#L956-L1001) The decision is sent with request correlation/content, receipt destination is checked, removal happens only afterward, and failures stay actionable.
- [`examples/033-the-office-demo-sh/web/src/main.ts:487-500`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts#L487-L500) Epoch advancement expires old-office approvals and prevents a late old receipt clearing current state.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Rejected sends, duplicate clicks, same-decision retry, missing runtime, late receipt after teardown, expired requests and deny receipts passed without unhandled rejection.

<a id="f09"></a>

### F09: Access-control callbacks report and restore topology inconsistently

**Severity:** high. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** audit-F-probes.cjs proves four concrete cases: all unwires reject yet finance is marked revoked; restart rewires all peers but finance remains in revokedAgents; restore arriving while revoke awaits is discarded; revoke gate+finance then restore finance rewires finance-gate while gate remains marked revoked.

- [`examples/033-the-office-demo-sh/web/src/main.ts:320-353,455-467,569-573`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) All wire/unwire exceptions are swallowed before marking success; restore rewires every original edge even when the other endpoint is revoked; the set is updated only after awaits and is retained when a fresh runtime rewires all members.
- [`examples/033-the-office-demo-sh/web/src/events.ts:67,144-157`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts) The async access-control callback is typed/invoked as void, allowing revoke and restore events in the same drain to interleave.

**Independent challenge:** All four claimed consistency failures reproduced independently: swallowed unwire failures still mark revoked; restart retains revoked labels despite new wiring; same-drain revoke/restore loses restore; restoring one of two revoked adjacent members reconnects the still-revoked peer. These are demo topology/projection bugs, not proof of actual credential revocation or a production authorization boundary. Real full topology operation was not validated successfully in this challenge.

**Accepted correction:** Serialize topology transitions; only realize edges permitted by both endpoints' desired access state; distinguish complete, partial and failed operations instead of treating all exceptions as idempotent success; reset/reconcile runtime-scoped labels on restart. Do not claim the wire operation revokes credentials beyond this in-memory topology.

- [`examples/033-the-office-demo-sh/web/src/main.ts:320-353`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts#L320-L353) Transition checks precede awaits, errors are swallowed unconditionally, restore ignores the other endpoint's revoked state, and state/messages are committed as success.
- [`examples/033-the-office-demo-sh/web/src/main.ts:455-467,494-495,569-573`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) Restart clears polling/subscriptions and initializes/rebuilds topology, but never clears or reconciles revokedAgents.
- [`examples/033-the-office-demo-sh/web/src/events.ts:67-72,144-157`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/events.ts) The callback accepts void; drain invokes access actions without awaiting them, so asynchronous transitions overlap.

**Implementation (fixed):** Introduce serialized OfficeTopology transitions with per-edge acknowledged/unknown projection and both-endpoint desired-state intersection. Report complete/partial/failed operations with details instead of swallowing failures. Reconcile uncertain edges on retry, reset topology per successful bootstrap, and describe these as in-memory demo edges rather than credential revocation.
Changed: `examples/033-the-office-demo-sh/web/src/topology.ts`, `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/src/agents.ts`, `examples/033-the-office-demo-sh/README.md`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Actual topology functions and event callback: same-drain revoke/restore serialization, total/partial unwire failures, failed wire restore and retry, adjacent blocked endpoints, edge sets plus reported states, and restart reconciliation.
- `cd examples/033-the-office-demo-sh/web && npm run test:offline`: **fail** Full real topology remains blocked by first wire's existing 30-second ambiguous in-process-delivery timeout. No fake topology success or runtime repair is claimed.

**Independent fix review (fixed):** Topology changes are serialized and compute each edge from both endpoints' desired disconnection state. Acknowledged and unknown edges are distinguished, failed operations do not claim success, retries reconcile uncertainty, and new runtime topology gets a fresh projection.
- [`examples/033-the-office-demo-sh/web/src/topology.ts:11-49`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/topology.ts#L11-L49) Promise tail serializes operations; per-edge state changes only on acknowledgment; failed edges become unknown; both endpoint sets determine desired connectivity.
- [`examples/033-the-office-demo-sh/web/src/main.ts:335-358,472-474,666-671`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts) Host reports complete/partial/failed details; teardown drains topology work and successful bootstrap installs a fresh tracker.
- [`examples/033-the-office-demo-sh/web/src/agents.ts:65-69`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/agents.ts#L65-L69) Tools are explicitly demo comms-topology requests, not network credential revocation.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Same-drain revoke/restore, total/partial failures, retries, dual-endpoint blocking and restart reset pass. Independent topology probes also pass. Final real-WASM full-page verification additionally confirms all 26 bidirectional base edges and lifecycle restoration; fault-injection cases remain controlled-runtime tests.

<a id="f10"></a>

### F10: Open Records and Graph tabs never refresh when Archivist writes records

**Severity:** medium. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** The source probe opens empty Records, inserts a valid record, and observes getRecordCount()==1 while the footer still says 0 RECORDS and the content lacks the record. The graph's captured elements likewise remain unchanged after another upsert. Reopening the tab reveals the data.

- [`examples/033-the-office-demo-sh/web/src/knowledge.ts:24-64,75-88,220-225`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/knowledge.ts) upsertRecord only mutates the Map. Rendering happens solely when showCaseFiles/showGraph is called.
- [`examples/033-the-office-demo-sh/web/src/main.ts:259-284,302-305`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) Those render methods are called on tab switches; the archive callback only calls upsertRecord, with no refresh of the visible view.

**Independent challenge:** Archive writes update the data Map but neither already-open view. This is not a failed write: reopening the tab reveals the data. The claimed hidden/active-tab problem is relevant to implementing refresh because hideKnowledgeBase currently leaves activeTab='cases'; an unconditional isKBVisible check would falsely report the hidden view as active.

**Accepted correction:** Notify/refresh the currently visible knowledge view after upsert, including footer and updates to existing records. Represent hidden state separately and avoid rebuilding inactive graph views.

- [`examples/033-the-office-demo-sh/web/src/knowledge.ts:19-24,24-64,71-88,220-225`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/knowledge.ts) upsertRecord mutates only records; renderCaseFiles/renderGraph are invoked by show methods, while hiding resets the tab to cases.
- [`examples/033-the-office-demo-sh/web/src/main.ts:259-284,302-305`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) The caller renders on tab switches but only calls upsertRecord on archive events.

**Implementation (fixed):** Track hidden knowledge state separately. Upsert refreshes only the visible Records/footer or Graph view; hidden views defer rendering until shown.
Changed: `examples/033-the-office-demo-sh/web/src/knowledge.ts`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Actual DOM/Cytoscape: visible record insert/summary/entity/footer update, real graph node/edge insert/update, no hidden graph/record rebuild, and cabinet opens current Records.

**Independent fix review (fixed):** Upserts refresh the currently visible Records/footer or Graph view. Hidden state is explicit and clears DOM references; inactive graph work is not rebuilt.
- [`examples/033-the-office-demo-sh/web/src/knowledge.ts:20-22,66-98`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/knowledge.ts) Visibility and selected tab are distinct; successful upsert dispatches only to the visible renderer.
- [`examples/033-the-office-demo-sh/web/src/main.ts:274-303`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts#L274-L303) Tab switching first hides prior knowledge state and then selects the correct current view.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Visible insert/update/footer, real Cytoscape node/edge updates, hidden no-rebuild behavior and cabinet reopen passed.

<a id="f11"></a>

### F11: Concurrent incident replies are attributed to whichever incident was opened last

**Severity:** medium. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** Create incident A, append its initial message, create B, then append an IT reply concerning A through the same null-ID callback path. audit-F-probes.cjs confirms the reply is stored under B.

- [`examples/033-the-office-demo-sh/web/src/incidents.ts:24-35,38-60`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/incidents.ts) New incidents are unshifted; addMessage(null, ...) selects the first active incident. There is no message correlation.
- [`examples/033-the-office-demo-sh/web/src/main.ts:297-300,666-667,702-704,888`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) Every event callback and human decision passes null, and injected scenario/chat IDs returned by createIncident are discarded.

**Independent challenge:** The current grouping fabricates attribution to the newest active incident. Initial user/scenario messages are correctly appended immediately after creation, but delayed agent replies are not correlated. There is no support in the displayed event contract for recovering the right incident from these null calls; a global log is a valid bounded fix and preferable to another heuristic.

**Accepted correction:** Either carry an actual end-to-end correlation value and preserve it, or use an honest chronological activity log with uncorrelated replies outside incident-specific ownership. Do not infer correlation from newest incident or text similarity.

- [`examples/033-the-office-demo-sh/web/src/incidents.ts:24-35,38-60`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/incidents.ts) New incidents are unshifted; null selects the first active incident rather than a correlated one.
- [`examples/033-the-office-demo-sh/web/src/main.ts:297-300,666-667,702-704,888`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) The event callback and approval path always pass null; scenario/chat creation discards the returned incident ID.
- [`examples/033-the-office-demo-sh/web/src/agents.ts:208-224`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/agents.ts#L208-L224) The output schema carries headline/category, not authoritative incident correlation.

**Implementation (fixed):** Use chronological observed activity rather than fabricated incident causality. Initial scenario/chat entries carry explicit incident IDs; null-correlated summaries, sends, approval decisions and topology effects remain visible as UNCORRELATED ACTIVITY.
Changed: `examples/033-the-office-demo-sh/web/src/incidents.ts`, `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Two actual scenario buttons and Enter-submitted chat are interleaved with delayed summary, topology effect and approval delivery. Each incident owns only its initial explicit input; replies/effects remain chronological uncorrelated log entries.

**Independent fix review (fixed):** Only initiating UI actions carry an explicit incident ID. Uncorrelated summaries, routing, approvals and topology effects remain chronological visible activity without falsely attaching to the newest incident.
- [`examples/033-the-office-demo-sh/web/src/incidents.ts:39-62,92-123`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/incidents.ts) A null incident never resolves through getActiveIncident; activity preserves observation order and renders UNCORRELATED ACTIVITY.
- [`examples/033-the-office-demo-sh/web/src/main.ts:307-310,747-749,783-784,992-993`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts) Initial scenario/chat calls supply their actual IDs, while event callbacks and approval effects do not invent correlation.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Two scenarios plus chat each retained only the initiating message; delayed agent replies, approval and topology output remained visible and uncorrelated in observation order.

<a id="f12"></a>

### F12: Startup reports LIVE even when every member failed to spawn or subscribe

**Severity:** high. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** The deterministic startup probe returns ten failed spawn results, rejects all wire calls and all subscriptions, then verifies the UI still reports LIVE and running=true. Separately, real-WASM happy-path startup passed with ten actual members, so this finding is specifically the failed/partial-start contract.

- [`examples/033-the-office-demo-sh/web/src/main.ts:561-572,591-600,644-647`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) The mob_spawn result is discarded. Wire and subscribe failures are console-only, and startup unconditionally sets running=true and LIVE.
- [`meerkat-web-runtime/src/lib.rs:2086-2130`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L2086-L2130) mob_spawn resolves a JSON array of typed per-spec results, explicitly serializing failures as entries rather than rejecting the whole promise.
- [`meerkat-contracts/src/wire/mob.rs:1288-1291,1325-1340`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-contracts/src/wire/mob.rs) The batch entry has a status field and failed result payload that the caller must inspect.

**Independent challenge:** Both mock and real WASM failure paths reach LIVE with no usable subscriptions. The real all-failed batch was induced by substituting an absent profile at the spawn boundary, not by falsely assuming that mob_spawn rejects its entire batch. A second full-topology run with synthetic auth failures had ten spawned roster entries but zero subscriptions and still reached LIVE; therefore roster count and Running status alone are not adequate counterevidence. A successful provider-backed fully wired office was not established.

**Accepted correction:** Inspect every spawn entry and require the promised roster, required wires and required subscriptions before LIVE. On failure, report actionable stage/member details, clean up the partial runtime or expose an explicitly non-live partial state, and provide the visible retry path from F05. Do not turn swallowed errors into a new optimistic readiness check.

- [`examples/033-the-office-demo-sh/web/src/main.ts:561-600,644-651`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) Spawn result is ignored; wire and subscription exceptions are only logged; running and LIVE are then set unconditionally.
- [`meerkat-web-runtime/src/lib.rs:2086-2125`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L2086-L2125) The resolved return value serializes individual successful and failed spawn results, requiring caller inspection.
- [`meerkat-contracts/src/wire/mob.rs:1284-1291,1314-1340`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-contracts/src/wire/mob.rs) Each typed result entry has a matching spawned/failed status and payload.

**Implementation (fixed):** Require all ten ordered spawned member results, canonical peer targets, every required wire, injected context and all ten subscription handles before LIVE. Failed stages report member/edge details, tear down partial subscriptions/mob/runtime, and expose visible Retry. Full real-WASM verification remains OPEN pending shared comms-drain repair; no topology/kickoff workaround is retained.
Changed: `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/src/events.ts`, `examples/033-the-office-demo-sh/web/src/types.ts`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`, `examples/033-the-office-demo-sh/web/tests/provider-fixture.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Actual startup path tested with all-failed and mixed real-shaped spawn results, wire rejection, Gate/Archivist subscription rejection, canonical target failure, and healthy controlled runtime (10 subscriptions, 26 realized edges). Cleanup and visible retry asserted.
- `cd examples/033-the-office-demo-sh/web && npm run test:offline`: **fail** With valid synthetic provider responses (not blocked/401 calls), all ten member snapshots are active/healthy/idle with kickoff Started and tokens_used 40; ten actual member subscriptions succeed. First triage ↔ it-dept wire still times out admitted-but-undrained. Causal read finds ensure_mob_comms_drain is explicitly a no-op under wasm32 (actor.rs:2441-2492). Page cleans up and exposes Retry; strict full-readiness assertion remains failing. Full evidence: fix-F-runtime-repro.json.

**Independent fix review (fixed):** The optimistic-readiness defect is corrected: spawn outcomes, canonical targets, every wire and ten subscriptions must succeed before LIVE. Earlier real missing-stream runs proved fail-closed cleanup/Retry. Two independent final ordinary runs now reach actual ten-member LIVE with all edges/subscriptions, successful bounded synthetic provider work and complete lifecycle checks.
- [`examples/033-the-office-demo-sh/web/src/main.ts:650-676,714-732`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts) Readiness prerequisites are awaited and failed stages include specific member/edge errors; catch cleans the partial runtime and restores Retry.
- [`examples/033-the-office-demo-sh/web/src/main.ts:468-516`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts#L468-L516) Partial subscriptions are closed and runtime destroyed; subscription failures are not swallowed.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** All-failed and mixed spawn, wire failure, Gate/Archivist subscription failures and peer-target failure all stayed non-LIVE and cleaned partial state. Controlled healthy bootstrap required 10 subscriptions and 26 wires.
- `npm --prefix examples/033-the-office-demo-sh/web run test:offline`: **pass** Two independent ordinary rebuilt-page runs pass LIVE, exact ten-member roster, ten unique peers, 26 bidirectional edges, ten subscriptions and stop/resume/destroy; zero errors/pending responses. Historical fail-closed cleanup/Retry evidence is retained.

<a id="f13"></a>

### F13: Usage instructions describe controls and full-message views that do not exist

**Severity:** low. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** Read the generated HTML controls and filing-cabinet handler. audit-F-probes.cjs renders a message containing a unique full-content marker and proves the marker is absent from the log HTML.

- [`examples/033-the-office-demo-sh/README.md:66-76`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/README.md#L66-L76) Usage directs users to '+ Event', an 'Incidents panel' with the 'full message tree', and a filing cabinet that opens the graph.
- [`examples/033-the-office-demo-sh/web/src/main.ts:66-69,89-93,255-256`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) Actual UI uses six scenario buttons and LOG/RECORDS/GRAPH tabs; the filing-cabinet callback opens Records, not Graph.
- [`examples/033-the-office-demo-sh/web/src/incidents.ts:104-114`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/incidents.ts#L104-L114) Only truncated headline previews are rendered. Stored content is never displayed or expandable, so no full message tree is available.

**Independent challenge:** The usage instructions name absent or mismatched controls and promise a full-message tree the implementation does not render. There is no requirement to build a new tree UI to resolve documentation drift; revising the documentation to the existing preview log and tab layout is sufficient.

**Accepted correction:** Align Usage with visible startup/setup behavior after F05, actual scenario buttons and tabs, the cabinet's Records destination, and the preview-only log (or whichever verified logging shape F11 intentionally adopts). Do not promise an unimplemented full-message view.

- [`examples/033-the-office-demo-sh/README.md:67-76`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/README.md#L67-L76) Usage names + Event, an Incidents full-message tree, and a cabinet opening the knowledge graph.
- [`examples/033-the-office-demo-sh/web/src/main.ts:66-69,89-93,255-256`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/main.ts) There are six scenario buttons, LOG/RECORDS/GRAPH tabs, and the cabinet opens Records.
- [`examples/033-the-office-demo-sh/web/src/incidents.ts:93-119`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/033-the-office-demo-sh/web/src/incidents.ts#L93-L119) Rendering uses headline truncated at 70 characters, not stored content, with no full-content expansion.

**Implementation (fixed):** Rewrite Usage to match PRESS START/key dialog, visible Start/Retry/Restart, six named scenario buttons, LOG/RECORDS/GRAPH, cabinet-to-Records, preview-only uncorrelated logs, retryable approvals and real execution pause. Document demo-only topology and strict offline validation limits.
Changed: `examples/033-the-office-demo-sh/README.md`, `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/package.json`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`.
- `cd examples/033-the-office-demo-sh/web && npm test`: **pass** Actual handler/DOM checks exercise named startup/configuration/retry controls, six scenario buttons, all three tabs, cabinet Records destination, preview logging and approval actions. The built page separately exercises real initial key setup and visible failure Retry.

**Independent fix review (fixed):** Usage matches the current setup and Start/Retry/Restart controls, six scenarios, LOG/RECORDS/GRAPH tabs, cabinet destination and preview-only uncorrelated activity. It no longer promises a full-message tree or credential revocation.
- [`examples/033-the-office-demo-sh/README.md:70-103`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/README.md#L70-L103) Revised usage and limitations name actual controls, receipt semantics, pause behavior and demo topology.
- [`examples/033-the-office-demo-sh/web/src/main.ts:272-303,832-890,913-1001`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts) Visible controls/tab paths and approval semantics match documentation.
- [`examples/033-the-office-demo-sh/web/src/incidents.ts:92-123`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/incidents.ts#L92-L123) The log is honestly described as chronological headline previews.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** Actual DOM confirmed six scenario buttons, Start/Pause and all three documented tabs, cabinet-to-Records and chronological uncorrelated activity.

<a id="f-w4-01"></a>

### F-W4-01: Office appends static admin instructions after conversation start on models that require a leading System prefix

**Severity:** high. **Verdict:** confirmed. **Examples:** 033.

**Original proof:** Example bootstrap/provider-input incompatibility, not stale JS session capture, overlapping init, a subscription parser bug, or a need to resurrect missing runtime actors.; Terminal failure: Failed (LlmFailure): LLM failure terminal turn: LLM error (anthropic): Invalid input shape: Anthropic model claude-sonnet-4-6 cannot represent System message at transcript index 5; this model requires System rows to form a leading prefix; matched single-member no-append control completes4 synthetic provider requests and retains a live subscription.

- `examples/033-the-office-demo-sh/web/src/main.ts:650-691` (reviewed worktree before F-W4-01 correction) Autonomous spawn and all26 wires precede late ordinary System append to every member; required subscriptions are opened afterward.
- `examples/033-the-office-demo-sh/web/src/agents.ts:10-20,219-245` (reviewed worktree before F-W4-01 correction) Every role already incorporates CYCLE_MODEL into its preloaded inline skill. This is the canonical existing place to include static instructions before the first turn.
- [`meerkat-web-runtime/src/lib.rs:2375-2400,2831-2854`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs) System append resolves member bridge session and calls the concrete session service. Subscription separately reads that same concrete service; no JS event processing occurs before NotFound.
- [`meerkat-session/src/ephemeral.rs:4366-4397,5162-5175,6016-6025,7923-7940`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/ephemeral.rs) Append is a normal actor command that mutates the transcript and returns status. subscribe_session_events_raw fails only for absent live-map entry.
- [`meerkat/src/service_factory.rs:378-392`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/service_factory.rs#L378-L392) FactoryAgent appends the ordinary System idempotently at the current transcript tail; it does not move it into a leading prefix.
- [`meerkat-models/src/capabilities/anthropic.rs:404-434`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-models/src/capabilities/anthropic.rs#L404-L434) claude-sonnet-4-6 supports_mid_conversation_system_messages=false. This is a deliberate model capability, not a missing WASM adapter.
- [`meerkat-anthropic/src/client.rs:494-534`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-anthropic/src/client.rs#L494-L534) project_anthropic_system_message_order rejects a non-leading System for that capability with the EXACT message captured from the real WASM run_failed event.
- [`meerkat-core/src/lifecycle/core_executor.rs:383-404`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/lifecycle/core_executor.rs#L383-L404) Specific AgentError::TerminalFailure maps to the corresponding CoreExecutorError::TerminalFailure.
- [`meerkat-runtime/src/runtime_loop.rs:3692-3701,6980-7011,3177-3208`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-runtime/src/runtime_loop.rs) Error-shaped terminal without a committed receipt/session witness enters generated stop, then terminalizes and calls executor cleanup. This explains lost live session after the prior run_failed event, not a frontend subscription malfunction.
- [`meerkat-mob/src/runtime/provisioner.rs:9864-9917,10196-10209,10307-10316`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/runtime/provisioner.rs) Mob executor maps the service failure and delegates post-stop cleanup to the exact actor witness; cleanup calls discard_live_session_actor_under_runtime_turn_boundary.
- [`meerkat-session/src/ephemeral.rs:3679-3706`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/ephemeral.rs#L3679-L3706) Exact cleanup revokes and removes only the matching live actor from the sessions map.
- [`meerkat-mob/src/profile.rs:391-420`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/profile.rs#L391-L420) Profiles deliberately have no system_prompt field. The minimal example correction must use existing initial role skills, not invent a profile key.

**Independent challenge:** The missing subscription is a consequence, not the originating defect. Office appends a static policy as an ordinary chronological System row after autonomous kickoff has already produced conversation history. claude-sonnet-4-6 explicitly lacks mid-conversation System support; its actual provider projection rejects that transcript before another HTTP call. Independently rerunning the one-member append/followup case produced the exact typed LlmFailure at transcript index 5, an absent subscription and broken member, with HTTP count remaining 2. The identical no-append control completed the second turn with 4 HTTP calls and a live active member. Moving the SAME policy into existing initial role skills and removing only the late bootstrap append passes the genuine-WASM ten-member counterfactual, including unchanged topology/subscription/lifecycle assertions. The actual landed/built page remains unverified until an authorized source repair is applied.

**Accepted correction:** owner: examples/033-the-office-demo-sh/web only; production_paths: src/agents.ts; src/main.ts; related_validation_paths: tests/regression.cjs; README.md; correction: Place the exact existing static policy text in the initial shared role-skill instructions, through CYCLE_MODEL or a single explicit constant interpolated there, so all ten roles receive it before their first turn.; Remove only the redundant late admin-policy mob_append_system_context bootstrap loop from main.ts.; Keep the selected models, all ten spawn-result checks, canonical peer map, all 26 wire validations, all ten required subscriptions, fail-closed readiness/cleanup and actual stop/resume controls unchanged.; Document static policy placement if relevant, without claiming all runtime System injection is invalid.; prohibited_scope_expansion: Do not remove, rename, restrict or change public mob_append_system_context or chronological System-message semantics.; Do not change model capability facts or provider preflight rules, switch the demo/test model to evade the failure, or force-resurrect failed actors.; Do not drop/reword the policy, reduce its role coverage, invent profile.system_prompt, omit subscriptions, weaken LIVE assertions, add arbitrary sleeps or extend timeouts.; Do not infer a general prohibition on legitimate dynamic context injection. Supporting models retain their existing capability-gated behavior; this correction concerns a static startup policy unnecessarily appended late.

- `examples/033-the-office-demo-sh/web/src/main.ts:650-691` (reviewed worktree before F-W4-01 correction) Autonomous spawn and all explicit wires precede the static admin-policy mob_append_system_context loop. The exact policy text is already known before creation, and the loop targets all AGENT_IDS.
- `examples/033-the-office-demo-sh/web/src/agents.ts:10-20,42-197,219-245` (reviewed worktree before F-W4-01 correction) Every one of the ten role skills incorporates CYCLE_MODEL, and buildOfficeDefinition preloads the corresponding inline skill for each profile before member creation. This existing initial instruction seam can carry the same policy without adding an unsupported profile field.
- [`meerkat-web-runtime/src/lib.rs:2373-2400`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L2373-L2400) mob_append_system_context is an ordinary System-append API which lowers to the concrete session service; it does not promise to move context before already committed conversation.
- [`meerkat/src/service_factory.rs:378-393`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/service_factory.rs#L378-L393) The agent append control calls append_system_message_idempotent on the current session transcript, preserving chronological System semantics.
- [`meerkat-models/src/capabilities/anthropic.rs:404-434`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-models/src/capabilities/anthropic.rs#L404-L434) The selected claude-sonnet-4-6 model explicitly declares supports_mid_conversation_system_messages=false.
- [`meerkat-anthropic/src/request_support.rs:19-26,45-62`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-anthropic/src/request_support.rs) The capability is catalog-owned. Existing supporting models, including claude-opus-4-8/claude-opus-5/claude-fable-5, are explicitly distinguished from sonnet-4-6; there is no universal ban on dynamic System context.
- [`meerkat-anthropic/src/client.rs:494-534`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-anthropic/src/client.rs#L494-L534) Provider projection accepts leading System rows, handles later rows when the capability is supported, and otherwise emits the EXACT InvalidInputShape message independently observed from the pinned real WASM.
- [`meerkat-session/src/ephemeral.rs:5162-5175`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/ephemeral.rs#L5162-L5175) The later stream-not-found error is from absence of the live SessionHandle, not a frontend envelope or polling mismatch.

**Implementation (fixed):** Preserve the exact static Boss/Outsider policy in OFFICE_ADMIN_POLICY and include it once in every initial CYCLE_MODEL role skill. Remove only the late bootstrap ordinary-System append loop that made Sonnet4.6 transcripts unrepresentable on later turns. Public append API, model capability facts, selected model, runtime authority, ten-member/26-edge topology, mandatory subscriptions and real pause/resume semantics remain unchanged. Fresh independent causal adjudication is challenge-F-W4-01-update.json; original F01-F13 entries below/above retain their historical evidence rather than being rewritten.
Changed: `examples/033-the-office-demo-sh/web/src/agents.ts`, `examples/033-the-office-demo-sh/web/src/main.ts`, `examples/033-the-office-demo-sh/web/tests/regression.cjs`, `examples/033-the-office-demo-sh/web/tests/provider-fixture.cjs`, `examples/033-the-office-demo-sh/README.md`.
- `npm --prefix examples/033-the-office-demo-sh/web run typecheck`: **pass** Actual final TS source passes tsc --noEmit.
- `npm --prefix examples/033-the-office-demo-sh/web run build`: **pass** Built the actual production page (index-DoHmSrkU.js), not merely the in-memory counterfactual. SDK/public/dist all independently hash to1beaf9f89ba7c43bb7e5e2c0af83774cbbf5a1f140500d628d56ffa83a0ae7d1. Existing Vite chunk-size warning only; no Rust/WASM build.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** All17 existing actual TS/DOM regression groups pass. Added real-definition assertion pins the original policy byte-for-byte and exactlyonce in the inline skill used by each of10profiles. Controlled startup's mob_append_system_context now throws if invoked, proving startup no longer depends on a late append. All prior failure/readiness/topology assertions retained.
- `npm --prefix examples/033-the-office-demo-sh/web run test:offline`: **pass** Initial ownerpasses used80/75requests and were superseded by independently observed80-cap failures; both are retained in budget_failure_history. After parent-authorized finite-workload correction, three consecutive ordinary built-page runs pass all10members/26edges/10subscriptions/LIVE/stop/admission/resume/destroy, with exact identity/phase accounting of1control+10initialmain+19batchedkickoffmain+29extractions=59requests representing ALL52directed terminal notices. Each directed sender/receiver pair occurs once; unknown work, repeatednotice delivery and extractions without a newmain are rejected. All10initialpolicycarriers, finalfixtureErrors===0, pendingresponses===0 remain mandatory. Bound125 derives from1+2*(10+2*26), fixed180s unchanged; no productlimit/model/runtime change.
- `git diff --check -- examples/033-the-office-demo-sh && node --check examples/033-the-office-demo-sh/web/tests/regression.cjs && test ! -e examples/033-the-office-demo-sh/web/.regression-browser`: **pass** Whitespace/syntax clean; owned profile removed and no owned browser/build remains.

**Independent fix review (fixed):** The exact static policy is now included once in all ten initial role skills and no longer appended after kickoff. This preserves legitimate capability-dependent runtime append behavior. The independently justified finite125 fixture bound is enforced with actual per-role/phase and canonical directed-input accounting, not an error waiver. Two independent ordinary actual-built-page runs now pass full healthy startup/lifecycle and all settlement/error assertions. Prior missing-stream/cap80 failures remain explicitly preserved.
- [`examples/033-the-office-demo-sh/web/src/main.ts:650-677`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/main.ts#L650-L677) Current source retains all spawn/peer/wire readiness checks and now proceeds directly to mandatory subscriptions without the late policy append.
- [`examples/033-the-office-demo-sh/web/src/agents.ts:10-23`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/src/agents.ts#L10-L23) Exact original policy literal is exported and included once in initial shared role instructions.
- [`examples/033-the-office-demo-sh/web/tests/regression.cjs:34-46,474-561,665-685`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/tests/regression.cjs) Exact policy/all-ten-role assertions plus nonvacuous actual system-prefix coverage; every request classified, unexpected/repeated work rejected; finite graph-derived bound and final zero-pending/zero-error reconciliation all pass.
- [`examples/033-the-office-demo-sh/web/tests/provider-fixture.cjs:6-24`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/033-the-office-demo-sh/web/tests/provider-fixture.cjs#L6-L24) Finite positive configurable maximum preserves default80 and 180s timeout. Office supplies justified125; no unlimited response fallback.
- `npm --prefix examples/033-the-office-demo-sh/web run typecheck`: **pass** Independent check of actual landed source passed.
- `npm --prefix examples/033-the-office-demo-sh/web test`: **pass** All 17 deterministic groups plus exact initial ten-role policy checks pass; controlled late append stub would throw if startup still used it.
- `npm --prefix examples/033-the-office-demo-sh/web run test:offline`: **pass** Independently executed twice after latest handoff: 75 and 79 fulfilled requests, respectively 37/37 and39/39 main/extraction, one control, ten initial roles and all52 unique directed notices. All10members/26edges/10subs/LIVE/stop/rejected-admission/resume/destroy and zero fixture errors/pending responses pass on pinned1beaf9f8.
