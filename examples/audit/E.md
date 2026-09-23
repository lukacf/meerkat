# Evidence group E

[Audit index](../AUDIT.md)

<a id="e01"></a>

### E01: WebCM mob creation submits a rejected legacy provider_params shape

**Severity:** high. **Verdict:** confirmed. **Examples:** 032.

**Original proof:** node $AUDIT_ARTIFACTS/audit-E-probes.mjs captures the exact emitted definition and prints {reasoning_effort:'low'}. Static current Rust deserialization chain proves this is rejected before mob creation. No Rust/WASM executable was rebuilt or run.

- [`examples/032-wasm-webcm-agent/web/src/mob.ts:99-120,269-270`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/mob.ts) Every profile inherits provider_params: {reasoning_effort: 'low'}, and init passes that definition to mob_create.
- [`meerkat-web-runtime/src/lib.rs:1957-1959`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L1957-L1959) mob_create deserializes directly into MobDefinition and returns invalid_definition on failure.
- [`meerkat-mob/src/profile.rs:318-322,925-933`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/profile.rs) Profile.provider_params is typed ProviderParamsOverride; a regression test explicitly rejects legacy flat provider parameter bags.
- [`meerkat-core/src/lifecycle/run_primitive.rs:808-825`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/lifecycle/run_primitive.rs#L808-L825) ProviderParamsOverride has deny_unknown_fields; reasoning_effort is not an allowed top-level field.

**Independent challenge:** Real current WASM rejects every captured WebCM profile definition, independently of provider assignment. The public error is invalid_definition / untagged ProfileBinding, not necessarily a diagnostic naming reasoning_effort. Deleting only provider_params makes mob_create succeed. A unique mob name was used consistently on both sides of each comparison to isolate successive runtime instances.

**Accepted correction:** Remove the unnecessary flat override from this example. If low reasoning is deliberately retained, use the existing typed ProviderParamsOverride contract and provider compatibility checks rather than inventing another flat bag.

- [`examples/032-wasm-webcm-agent/web/src/mob.ts:99-120,269-270`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/mob.ts) All four profiles inherit the same flat reasoning_effort bag and the emitted definition is passed directly to mob_create.
- [`meerkat-core/src/lifecycle/run_primitive.rs:802-825`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/lifecycle/run_primitive.rs#L802-L825) ProviderParamsOverride denies unknown fields; reasoning, not a flat reasoning_effort field, is part of the typed vocabulary.
- [`meerkat-web-runtime/src/lib.rs:1955-1965`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L1955-L1965) Deserializes MobDefinition before invoking mob creation.

**Implementation (fixed):** Removed the unnecessary legacy provider_params override. The real emitted definitions now deserialize in current 0.8.40 WASM for Anthropic-only, OpenAI-only, Gemini-only and all-provider assignments.
Changed: `examples/032-wasm-webcm-agent/web/src/mob.ts`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Real current WASM rejects the old flat override for each assignment and accepts all four fixed definitions. No members are spawned for this check; no provider traffic.

**Independent fix review (fixed):** The emitted profiles no longer contain the rejected flat provider_params bag. Current WASM accepts every provider assignment, and a counterfactual restoring only the old field is rejected. This proves definition compatibility, not full mob startup.
- [`examples/032-wasm-webcm-agent/web/src/mob.ts:102-131`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/mob.ts#L102-L131) Shared profile defaults use current model/tools/runtime fields and omit the legacy override.
- [`examples/032-wasm-webcm-agent/web/tests/regression.test.mjs:211-234`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/tests/regression.test.mjs#L211-L234) Four actual emitted definitions are accepted by WASM; four old-shape counterfactuals are rejected.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final 1beaf9f8 artifact: E01/E19 definition subtest passes (4 accepted/4 counterfactual rejected), and full startup now passes. Entire suite 8/8.

<a id="e02"></a>

### E02: Both examples read the removed run_failed.error field

**Severity:** high. **Verdict:** partial. **Examples:** 031, 032.

**Original proof:** audit-E-probes.mjs feeds a current-shaped run_failed envelope with error_report.message='synthetic auth failure'. 031 returns {events:1,errors:[]}; 032 renders only 'Run failed'.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:61-63`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts#L61-L63) Failures are accumulated only when event.error exists.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:160-194,231-258`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) The stop-and-show-auth-error branch depends on those accumulated failures; otherwise absent orders become fabricated 50-aggression fallback moves.
- [`examples/032-wasm-webcm-agent/web/src/mob.ts:425-428`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/mob.ts#L425-L428) The renderer uses ev.error || 'Run failed', discarding the actual current diagnostic.
- [`meerkat-core/src/event.rs:2088-2098`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs#L2088-L2098) RunFailed carries error_report with its message/class; the old error string is deliberately absent.

**Independent challenge:** Both payload handlers read a removed field. However, the claimed current-envelope reproduction for 031 is inaccurate: genuine current envelopes are rejected earlier by the separate E-NEW-01 defect. The payload mismatch is independently proven by a compatibility-envelope fixture and will still break failures after envelope ingress is fixed. 032 directly loses the diagnostic as claimed.

**Accepted correction:** Read error_report.message and preserve typed failure information; collect failures during extraction as well as deliberation and prevent a total failed turn from silently resolving fallback combat. Do not count the independent envelope fix as part of E02.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:24-36,61-63`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts) Legacy source_id validation precedes the obsolete event.error read.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:160-194,219-227,231-258`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) The deliberation failure branch depends on collected errors; extraction discards the returned errors entirely.
- [`examples/032-wasm-webcm-agent/web/src/mob.ts:425-428`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/mob.ts#L425-L428) Uses ev.error or generic Run failed.
- [`meerkat-core/src/event.rs:2088-2098`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs#L2088-L2098) RunFailed owns error_report and has no legacy error string.

**Implementation (fixed):** Both event consumers display error_report.message, preserving the typed report in diplomacy replay failures and WebCM error-element metadata. Diplomacy collects extraction-phase failures too and refuses to resolve combat when failed work produced no parsed orders.
Changed: `examples/031-wasm-mini-diplomacy-sh/web/src/events.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/types.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/main.ts`, `examples/032-wasm-webcm-agent/web/src/mob.ts`, `examples/032-wasm-webcm-agent/web/src/stream.ts`, `examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Actual typed payloads retain diagnostics/class. Full real UI loop with synthetic runtime proves deliberation failure and extraction failure leave Turn 1 unchanged, show Error and never call narrator/combat progression. Actual current WASM run_failed bytes are accepted directly.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Real renderer receives canonical diagnostic and preserves error_report.class; unresolved cards are terminalized as errors.

**Independent fix review (fixed):** Both consumers read the actual error_report, retain its typed data, and show its message. Diplomacy accumulates extraction-phase failures and does not resolve fallback combat when all orders are absent and failures were observed.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:74-81`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts#L74-L81) Current run_failed and extraction_failed fields populate errors; typed run reports are retained.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:224-246,273-275`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) Send and drain failures enter allErrors in extraction; an entirely failed order set throws before resolution.
- [`examples/032-wasm-webcm-agent/web/src/mob.ts:447-451`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/mob.ts#L447-L451) Failure settles outstanding cards as errors and supplies the full report to appendError.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Final artifact: typed failure fixtures, UI total/extraction failures, real unchanged WASM run_failed ingress and full startup all pass; suite 8/8.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final artifact: typed diagnostic/error metadata regression and full startup pass; suite 8/8.

<a id="e03"></a>

### E03: Narrator polling reads a MobRun directly instead of the current run envelope

**Severity:** medium. **Verdict:** confirmed. **Examples:** 031.

**Original proof:** The offline probe supplies {run:{status:'completed',step_ledger:[...]}}. The example's tested status is undefined and its exit predicate is true. The same predicate exits on the first running response.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:283-299`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L283-L299) The poller checks result.status and result.step_ledger at the response root, then breaks for any status other than running/pending.
- [`meerkat-web-runtime/src/lib.rs:2706-2720`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L2706-L2720) mob_flow_status explicitly returns JSON {run: status}, including {run:null} for absence.
- [`sdks/web/src/mob.ts:1243-1259`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/sdks/web/src/mob.ts#L1243-L1259) The maintained SDK correctly unwraps record.run before reading status.

**Independent challenge:** The maintained SDK and raw WASM agree that run is nested. The example reads the root, so undefined status terminates polling even while the nested run is running. The ledger is genuinely an array; the finding does not need an invented ledger-shape change.

**Accepted correction:** Unwrap run and distinguish absent/nonterminal/terminal states; render the completed summarize output once and stop only at a genuine terminal state or explicit polling deadline.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:283-299`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L283-L299) Reads result.status and result.step_ledger without unwrapping run.
- [`meerkat-web-runtime/src/lib.rs:2706-2720`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L2706-L2720) Serializes {run: status}.
- [`sdks/web/src/mob.ts:1243-1262`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/sdks/web/src/mob.ts#L1243-L1262) parseMobFlowStatusResult unwraps record.run and handles null.
- [`meerkat-mob/src/run.rs:4802,6087-6094`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/src/run.rs) step_ledger is a Vec with output on StepLedgerEntry.

**Implementation (fixed):** Narrator polling unwraps the run envelope, waits through absent/pending/running states, and stops at completed/failed/canceled or the bounded 40-poll deadline. Completed summarize output renders once.
Changed: `examples/031-wasm-mini-diplomacy-sh/web/src/runner.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/main.ts`, `examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs`.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Real pollNarrative exercised for all accepted states and deadline; full UI turn loop receives nested completed flow and invokes narrator exactly once per completed round.

**Independent fix review (fixed):** Narrator polling unwraps run, waits through null/pending/running, stops on genuine terminal states and remains bounded at forty polls. The caller renders the returned narrative once.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/runner.ts:69-85`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/runner.ts#L69-L85) Nested run.status is authoritative, with explicit terminal branches and a fixed poll limit.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:299-308`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L299-L308) One pollNarrative result produces one narrator message and one narrator-feed entry.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Final artifact: null/pending/running/completed/failed/canceled/deadline cases, UI narrator flow and full real startup pass.

<a id="e04"></a>

### E04: Diplomacy compact summaries ignore the actual extraction terminal event

**Severity:** medium. **Verdict:** partial. **Examples:** 031.

**Original proof:** audit-E-probes.mjs sends RunCompleted(result='Messages sent; waiting.', extraction_required=true) followed by ExtractionSucceeded with valid headline/dispatches. The example emits zero summaries.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:83-117`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts#L83-L117) Structured summaries are parsed only from JSON.parse(run_completed.result); extraction_succeeded is never consumed.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts:91-119,133-151`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts) Faction profiles request output_schema headline/dispatches, so they exercise the extraction path.
- [`meerkat-core/src/agent/state.rs:6812-6838,6723-6741`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs) The main run emits RunCompleted with primary text, structured_output=None, extraction_required=true; validated extraction later emits ExtractionSucceeded carrying structured_output.
- [`meerkat-core/src/event.rs:2056-2085`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs#L2056-L2085) RunCompleted and ExtractionSucceeded/ExtractionFailed are distinct current contracts.

**Independent challenge:** Extraction terminal events and inline structured_output are ignored, but a real current envelope cannot reach that handler until independent E-NEW-01 is fixed. A model might coincidentally put JSON into the main result, so this is not proof that every possible completed run is blank; it is a deterministic failure of the supported structured carriers.

**Accepted correction:** Consume structured_output from its actual carrier and display extraction failure; prevent duplicate summaries. Do not rely on parsing the main natural-language result as the authoritative structured value.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:83-117`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts#L83-L117) Only JSON.parse(run_completed.result) produces summaries.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts:91-119,133-151`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts) Faction profiles enable the structured extraction path.
- [`meerkat-core/src/event.rs:2056-2085`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs#L2056-L2085) Separate RunCompleted structured_output and ExtractionSucceeded/Failed contracts.
- [`meerkat-core/src/agent/state.rs:6723-6741,6812-6838`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs) Main run commits before extraction; validated output is emitted later.

**Implementation (fixed):** Structured summaries consume structured_output from extraction_succeeded or run_completed, not natural-language result JSON. Extraction failures surface diagnostics, identical envelopes and duplicate completion carriers do not duplicate summaries.
Changed: `examples/031-wasm-mini-diplomacy-sh/web/src/events.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/types.ts`, `examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs`.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Current typed envelopes exercise separate extraction success/failure, inline output, ordinary unstructured completion, duplicate carrier and new-run reset through actual drainAllEvents and real UI sinks.

**Independent fix review (fixed):** The consumer accepts structured_output on both supported terminal carriers, reports extraction failure, and resets summary suppression on a new run. It no longer treats natural-language result JSON as the authoritative structured carrier.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:79-84,101-137`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts) Current extraction carriers feed compact summaries, with per-run suppression and canonical peer mapping.
- [`meerkat-core/src/event.rs:2055-2085`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-core/src/event.rs#L2055-L2085) The authoritative run_completed/extraction_succeeded fields match the new consumer.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Final artifact: current structured carriers, duplicate suppression, inline output, diagnostics and actual full-startup structured events pass.

<a id="e05"></a>

### E05: Pause, Resume, Step and Start can re-enter the same diplomacy turn loop

**Severity:** high. **Verdict:** confirmed. **Examples:** 031.

**Original proof:** audit-E-probes.mjs executes the actual bundled handlers with deterministic fake sleeps. A single Step sends all three TURN 2 planner prompts before pausing, despite only resolving turn 1. Pause followed by Resume before the suspended tick wakes sends all three TURN 1 prompts twice.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:130-173,312-313`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) tick has no in-flight owner and recursively launches the next tick before its own promise resolves.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:451-456`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L451-L456) Resume always calls tick immediately; Step calls tick and clears running only after it returns; Start is never disabled or generation-guarded.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:353,402-415`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) Starting again replaces runtime/global session state while older asynchronous tick work can still be suspended.

**Independent challenge:** The real handlers have no runner ownership, and the recursive launch happens before the Step continuation can clear running. Pausing only flips a flag and does not own or await the suspended tick. The start path similarly reinitializes shared runtime state without serialization.

**Accepted correction:** Use one owned campaign runner; separate one-turn execution from continuous scheduling. Serialize controls at defined safe boundaries, prevent overlapping Start initialization, and retire/close previous campaign resources before replacement. Fence suspended work against stale campaign identity.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:130-173,312-313,353,402-415,451-456`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) Shared globals, recursive fire-and-forget tick, and unconditional control-handler launches.
- [`meerkat-web-runtime/src/lib.rs:589-608,1468-1505`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs) Reinitialization installs replacement runtime state and clears the subscription registry.

**Implementation (fixed):** Introduced one owned CampaignRunner. Tick executes one turn only; Step is not recursive, rapid controls coalesce, Pause takes effect at a completed-turn boundary, and replacement awaits the old turn before closing its subscriptions/mobs and destroying its runtime. Tick captures its own session rather than consulting a replaced global after suspension.
Changed: `examples/031-wasm-mini-diplomacy-sh/web/src/runner.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/main.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/types.ts`, `examples/031-wasm-mini-diplomacy-sh/README.md`, `examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs`.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Controlled promises prove single startup/turn ownership and no old-run mutation. Browser-clock full UI tests double-Start, Pause, double-Step and replacement: one initialization, one round per Step, all nine old subscriptions closed and four mobs destroyed before second initialization.

**Independent fix review (fixed):** A single owned runner serializes initialization/replacement and turn execution. Step finishes one current/new turn without recursively scheduling; rapid controls share the active promise. Replacement waits for the captured old session's turn, then closes its resources.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/runner.ts:2-52`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/runner.ts#L2-L52) starting/active ownership, captured session, continuous flag and replacement sequencing prevent overlapping turn loops.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:117-131,148-150,437-444,480-490`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) The owned tick receives its session argument; replacement closes subscriptions/mobs/runtime; controls route through the runner.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Final artifact: runner races, real UI double Start/Step/resource replacement and full startup pass. Reviewer also independently executed paused replace/step race: exactly start:a,end:a,dispose:a and no stale work.

<a id="e06"></a>

### E06: Final-order extraction inherits an already-expired quiet timer

**Severity:** medium. **Verdict:** confirmed. **Examples:** 031.

**Original proof:** The actual tick in audit-E-probes.mjs reaches extraction at fake time 8100 ms. With no event in the first extraction poll it abandons extraction after 300 ms, then resolves fallback orders; the declared 20-second extraction deadline was never meaningfully offered.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:157-171,199-227`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) Deliberation ends after lastEventTime is over eight seconds old. Extraction reuses that timestamp with a six-second cutoff, without resetting it when sending the extraction prompts.

**Independent challenge:** The extraction deadline is nominally twenty seconds but the inherited idle timestamp has already aged past six seconds when deliberation ends. This is an independent timing defect, regardless of why no event arrived.

**Accepted correction:** Give extraction a fresh observation baseline and a bounded completion/wait policy for its newly queued work; do not reuse deliberation's expired idle clock. Avoid introducing an unlimited wait.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:157-171,199-227`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) A single lastEventTime is reused across deliberation and extraction.

**Implementation (fixed):** Extraction now observes its own bounded 20-second work window instead of reusing deliberation's expired idle clock.
Changed: `examples/031-wasm-mini-diplomacy-sh/web/src/runner.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/main.ts`, `examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs`.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Fake-clock helper tests cover delayed work and genuine bounded timeout. The actual UI/drain path queues canonical operator events 1200ms after extraction; all three aggression-100 orders reach narration instead of aggression-50 fallback.

**Independent fix review (fixed):** Extraction receives a fresh twenty-second observation deadline, independent of the deliberation quiet timer. The accepted fix remains bounded rather than turning the wait into an infinite loop.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/runner.ts:55-67`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/runner.ts#L55-L67) Deadline is initialized at waitForOrders entry; each poll is delayed and bounded.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:214-241`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L214-L241) The new observation window starts only after sending extraction work.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Final artifact: delayed-order receipt, genuine twenty-second timeout, delayed UI extraction and full startup pass.

<a id="e07"></a>

### E07: write_file reports success when directory creation or the write command fails

**Severity:** medium. **Verdict:** confirmed. **Examples:** 032.

**Original proof:** audit-E-probes.mjs supplies mkdir exitCode=1 and writeFile exitCode=1 ('Read-only file system'). The real registered callback returns {content:'Wrote /workspace/a b.txt',is_error:false}.

- [`examples/032-wasm-webcm-agent/web/src/tools.ts:100-108`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/tools.ts#L100-L108) Both vm.exec(mkdir...) and vm.writeFile return values are ignored; the callback unconditionally returns is_error:false and 'Wrote ...'.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:154-173`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/webcm-host.ts#L154-L173) writeFile returns ExecResult, including a nonzero exitCode; ordinary shell failure is not a rejected promise.

**Independent challenge:** Nonzero ExecResult is an ordinary resolved value, not an exception. The callback ignores both failures; the chunk helper also ignores intermediate status.

**Accepted correction:** Check directory creation and every write stage, stop at first failure, and return the failing output/exit status as a tool error. Do not fabricate success when a partial chunk failed.

- [`examples/032-wasm-webcm-agent/web/src/tools.ts:100-108`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/tools.ts#L100-L108) Unconditionally emits Wrote with is_error false.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:154-173`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/webcm-host.ts#L154-L173) Intermediate chunk command results are discarded.

**Implementation (fixed):** write_file checks directory creation and final write status. Large writes stop on failed staging/chunk/decode operations and return the actual failing ExecResult instead of fabricating success; scoped staging cleanup remains in finally.
Changed: `examples/032-wasm-webcm-agent/web/src/tools.ts`, `examples/032-wasm-webcm-agent/web/src/webcm-host.ts`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Original accepted-fix suite: actual callbacks/host functions test successful writes, failed mkdir, failed short write, failed intermediate chunk (no decode afterward), and serialized queue recovery after a rejected operation. Current whole-suite runtime blocker is recorded in coverage.
- `npm --prefix examples/032-wasm-webcm-agent/web run test:guest`: **pass** Real documented WebCM guest boots via actual WebCMHost. Twelve actual guest file write/read round trips succeed, including short and multi-chunk Unicode content. No LLM/runtime/provider requests.

**Independent fix review (fixed):** Directory and write exit codes now control tool success. Large writes stop at the first failed staging/chunk/decode operation and preserve its diagnostic rather than manufacturing Wrote success.
- [`examples/032-wasm-webcm-agent/web/src/tools.ts:100-114`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/tools.ts#L100-L114) Both mkdir and writeFile results are checked before returning success.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:157-181`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/webcm-host.ts#L157-L181) Short writes return actual ExecResult; chunked writes short-circuit nonzero stages and finally clean scoped staging.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final artifact: directory, short-write and intermediate-chunk failures, queue recovery and full mob startup pass.
- `npm --prefix examples/032-wasm-webcm-agent/web run test:guest`: **pass** Reviewer booted actual public WebCM assets: 12 file round-trips and 4 PTY commands passed, crossOriginIsolated=true, no diagnostics/off-origin requests.

<a id="e08"></a>

### E08: File tools interpolate literal paths as unquoted shell syntax

**Severity:** medium. **Verdict:** confirmed. **Examples:** 032.

**Original proof:** audit-E-probes.mjs invokes the actual callbacks and host helpers with /workspace/a b.txt. Captured commands include mkdir -p $(dirname /workspace/a b.txt), redirection to /workspace/a b.txt, and cat /workspace/a b.txt, rather than one literal filename.

- [`examples/032-wasm-webcm-agent/web/src/tools.ts:105-106`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/tools.ts#L105-L106) Parent creation uses mkdir -p $(dirname ${args.path}) without literal-path quoting.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:163,172,178`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/webcm-host.ts) Short writes, chunked writes, and reads interpolate path directly into redirection/cat commands.

**Independent challenge:** The file helpers promise literal filenames but emit shell syntax. This is demonstrated with an ordinary space, not an assumed escape from the sandbox.

**Accepted correction:** Centralize POSIX literal quoting for path operands, including parent directory handling and both write branches; use supported option delimiters where necessary. No host-sandbox redesign is justified by this finding.

- [`examples/032-wasm-webcm-agent/web/src/tools.ts:105-106`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/tools.ts#L105-L106) The dirname argument and resulting mkdir argument are unquoted.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:163,172,178`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/webcm-host.ts) Both redirections and cat embed raw path text.

**Implementation (fixed):** Centralized POSIX literal quoting for parent directories, read operands, short writes and large-write staging. Option-taking mkdir/rm use --; redirection operands are quoted. Staging stays beside the requested file, not a global temporary directory.
Changed: `examples/032-wasm-webcm-agent/web/src/tools.ts`, `examples/032-wasm-webcm-agent/web/src/webcm-host.ts`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Original accepted-fix suite: real writeFile/readFile functions round-trip short and chunked Unicode content through a synthetic local POSIX-shell transport for space, apostrophe, dollar/wildcard and dash-prefixed filenames. Directory listing verifies literal requested names. Current whole-suite runtime blocker is recorded in coverage.
- `npm --prefix examples/032-wasm-webcm-agent/web run test:guest`: **pass** Actual WebCM guest verifies all four literal filename classes with three contents each (12 round trips), including apostrophes, spaces, dollar/wildcard characters, and a dash-prefixed basename. Synthetic guest directory removed afterward.

**Independent fix review (fixed):** All file operands use shared POSIX single-quote escaping, and parent directories are calculated as strings rather than via interpolated shell command substitution.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:18-20,157-189`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/webcm-host.ts) Read, short write and every long-write stage quote the complete literal pathname, including apostrophes.
- [`examples/032-wasm-webcm-agent/web/src/tools.ts:106-108`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/tools.ts#L106-L108) mkdir receives a quoted literal parent after --.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final artifact: actual helpers pass literal spaces/apostrophes/dollar/glob/leading-dash tests; full startup passes too.
- `npm --prefix examples/032-wasm-webcm-agent/web run test:guest`: **pass** All four literal guest pathname forms round-tripped three different byte-sensitive payloads, including chunked Unicode.

<a id="e09"></a>

### E09: PTY shell framing loses non-newline output and is swallowed by trailing comments

**Severity:** medium. **Verdict:** confirmed. **Examples:** 032.

**Original proof:** audit-E-probes.mjs supplies realistic PTY output for printf payload: payload immediately followed by the delimiter. exec returns output:'' with exitCode:0. A harmless local /bin/sh probe of the exact same-line wrapper with 'printf payload # ordinary comment' produces payload but no completion marker.

- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:91-115,127-152`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/webcm-host.ts) The completion echo is appended on the same command line. Parsing drops the entire line containing the completion delimiter, including any command output preceding it.

**Independent challenge:** The parser drops the entire marker line and trims retained output. Appending the marker on the same shell source line also lets an ordinary trailing comment swallow it.

**Accepted correction:** Separate command framing from user shell source so the completion marker survives comments, preserve the original exit status, and extract output bytes before the marker without dropping a non-newline final line or meaningful whitespace.

- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:91-115,127-152`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/webcm-host.ts) Same-line completion echo; slice excludes marker line; trim destroys surrounding whitespace.

**Implementation (fixed):** Command framing encloses shell source in a separately terminated group with distinct start/end control markers. Parsing slices output before the end marker without trimming or discarding its final line and waits for the full status marker. readFile uses base64 transport to preserve file line endings/whitespace through PTY processing.
Changed: `examples/032-wasm-webcm-agent/web/src/webcm-host.ts`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Original accepted-fix suite: actual frame/parser and exec polling cover empty output, no-final-newline, whitespace, multiline comments, nonzero status and echo removal with a synthetic shell. Current whole-suite runtime blocker is recorded in coverage.
- `npm --prefix examples/032-wasm-webcm-agent/web run test:guest`: **pass** Real WebCMHost/guest confirms printf abc returns exactly abc without a trailing newline; meaningful whitespace and multiline trailing-comment output are exact; false preserves exit 1. Actual read_file preserves CRLF/LF/Unicode and no-final-newline content.

**Independent fix review (fixed):** Separate start/end framing survives trailing comments, records the command status, waits for a complete status marker and slices the entire preceding output without trimming final content.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:32-46,130-152`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/webcm-host.ts) The group closes on its own line; output is sliced before the end marker, preserving final non-newline output and whitespace.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:184-189`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/webcm-host.ts#L184-L189) File reads use base64 so PTY newline normalization cannot change file contents.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final artifact: all five local shell/framed-host output/status fixtures and full-startup case pass.
- `npm --prefix examples/032-wasm-webcm-agent/web run test:guest`: **pass** Actual PTY preserved printf without newline, leading/trailing whitespace, multi-line trailing comments and nonzero status.

<a id="e10"></a>

### E10: WebCM stream rendering reads removed prompt and result fields

**Severity:** medium. **Verdict:** confirmed. **Examples:** 032.

**Original proof:** audit-E-probes.mjs routes current-shaped RunStarted and ToolExecutionCompleted envelopes. No 'message received' card is created and the tool-result card receives an empty string despite a text block containing 'one'.

- [`examples/032-wasm-webcm-agent/web/src/mob.ts:390-396,402-410`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/mob.ts) tool_execution_completed reads ev.result and run_started reads ev.prompt.
- [`meerkat-core/src/event.rs:2048-2053,2239-2250`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs) Current events carry RunStarted.input and ToolExecutionCompleted.content: Vec<ContentBlock>.
- [`meerkat-core/src/types.rs:626-651`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/types.rs#L626-L651) RunInput is a typed Content/PendingToolResults enum, not a prompt string field.

**Independent challenge:** WebCM routes payloads without 031's envelope gate, so the old prompt/result reads directly fail current events. PendingToolResults genuinely has no user prompt and should not be turned into one.

**Accepted correction:** Project display text from typed input/content blocks and preserve the no-prompt continuation variant. Reuse the content projection for the ToolResultReceived fallback when its execution event is absent.

- [`examples/032-wasm-webcm-agent/web/src/mob.ts:390-410`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/mob.ts#L390-L410) Reads ev.result and ev.prompt.
- [`meerkat-core/src/event.rs:2048-2053,2239-2250`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs) RunStarted.input and ToolExecutionCompleted.content are authoritative.
- [`meerkat-core/src/types.rs:623-651`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/types.rs#L623-L651) RunInput uses kind:content or kind:pending_tool_results.

**Implementation (fixed):** Projects display text from RunInput::Content and typed text content blocks. PendingToolResults stays prompt-less. Tool execution and fallback result-received events share the same text projection.
Changed: `examples/032-wasm-webcm-agent/web/src/mob.ts`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Actual routeEnvelope and DOM renderer test scalar content, text-block input, pending continuation without fabricated prompt, mixed image/text tool content and result-received fallback.

**Independent fix review (fixed):** The consumer projects RunInput::Content and current typed content blocks; pending_tool_results creates no fabricated prompt. Both result event variants use the same projection.
- [`examples/032-wasm-webcm-agent/web/src/mob.ts:35-40,409-412,422-433`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/mob.ts) String and text-block input projection is shared with execution/completion results; only input.kind=content produces a prompt.
- [`meerkat-core/src/event.rs:2048-2053,2175-2180,2239-2256`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-core/src/event.rs) Current run input and tool-result content contracts match the implementation.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final artifact: scalar/block prompts, non-text omission, pending continuation, both tool-result carriers and strict startup pass.

<a id="e11"></a>

### E11: Multiple tool calls overwrite the single pending card

**Severity:** medium. **Verdict:** confirmed. **Examples:** 032.

**Original proof:** audit-E-probes.mjs routes requested shell id=a, requested read_file id=b, completed a, completed b. The first completion resolves the read_file card, the shell card is left unresolved, and the second completion has no card.

- [`examples/032-wasm-webcm-agent/web/src/mob.ts:27-32,336-341,385-395`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/mob.ts) PanelState retains only currentCard. Every ToolCallRequested replaces it, while completions ignore their id.
- [`meerkat-core/src/event.rs:2161-2166,2239-2249`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs) Requests and completion events both carry a canonical id suitable for correlation.

**Independent challenge:** The registry is a single card, whereas the core emits all ToolCallRequested events before dispatching the batch. Serializing VM execution cannot prevent this overwrite.

**Accepted correction:** Maintain a per-panel pending-card map keyed by call ID. Resolve only the matching request, handle paired/replayed result facts idempotently, and clear remaining cards honestly on run failure or timeout rather than showing success.

- [`examples/032-wasm-webcm-agent/web/src/mob.ts:27-32,336-341,385-395`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/mob.ts) Every request replaces currentCard and completion ignores id.
- [`meerkat-core/src/agent/state.rs:6040-6103`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L6040-L6103) Loops over requests emitting their unchanged call IDs before executing the batch.

**Implementation (fixed):** Each panel owns a pending-card map keyed by call ID and canonical event replay set. Only matching results resolve cards; paired result facts are idempotent. Timeouts/run failures/run completion without results remove spinners honestly as errors, not success.
Changed: `examples/032-wasm-webcm-agent/web/src/mob.ts`, `examples/032-wasm-webcm-agent/web/src/main.ts`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Real routing/rendering covers batched requests, both completion orders, paired/replayed events, reused IDs on new requests, per-call errors/timeouts, failed runs and missing-result completion; no permanent spinner or cross-assigned output.

**Independent fix review (fixed):** Each panel tracks outstanding cards by call ID and removes only the matched card. Envelope replay suppression and removing resolved entries make paired results idempotent; timeouts and missing terminal results are rendered as errors.
- [`examples/032-wasm-webcm-agent/web/src/mob.ts:31-32,345-357,378-382,404-419,442-451`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/mob.ts) Map-based card ownership replaces the single mutable slot; terminal paths settle all remaining spinners honestly.
- [`examples/032-wasm-webcm-agent/web/src/main.ts:132-151`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/main.ts#L132-L151) Orchestrator and all specialist panels receive separate maps/sets.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final artifact: completion orders, errors, paired result facts, replay, timeout, terminal cleanup and strict startup pass.

<a id="e12"></a>

### E12: Model-generated Markdown can execute HTML event handlers in the host page

**Severity:** high. **Verdict:** confirmed. **Examples:** 032.

**Original proof:** node $AUDIT_ARTIFACTS/audit-E-browser.mjs loads the real StreamRenderer in isolated headless Chrome and calls finalizeText with an img whose onerror only sets globalThis.auditMarker=1. The marker executes. All non-loopback browser traffic is blocked; no credentials or external data were used.

- [`examples/032-wasm-webcm-agent/web/src/stream.ts:42-46,61-66`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/stream.ts) marked.parse output is inserted into innerHTML without sanitization. Marked preserves raw HTML.
- [`examples/032-wasm-webcm-agent/web/src/stream.ts:178-185`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/stream.ts#L178-L185) The banner also interpolates model strings into innerHTML.
- [`examples/032-wasm-webcm-agent/web/src/main.ts:23-29,107-110`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/main.ts) Those displayed model strings can come from URL query parameters.

**Independent challenge:** The real browser executes benign HTML event handlers passed through marked and through model-label interpolation. Streaming deltas alone use textContent and are safe; the dangerous transition is finalization/banner rendering.

**Accepted correction:** Sanitize rendered Markdown with an appropriate maintained allowlist and safe link schemes, or explicitly disallow raw HTML with equivalent protection. Construct model-label markup using text nodes rather than interpolation.

- [`examples/032-wasm-webcm-agent/web/src/stream.ts:42-46,49-66,178-185`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/stream.ts) Final markup and interpolated model labels use unsanitized innerHTML.
- [`examples/032-wasm-webcm-agent/web/src/main.ts:23-29,107-110`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/main.ts) Model labels can be supplied by URL overrides.

**Implementation (fixed):** DOMPurify sanitizes final Markdown in both fresh and pending-text paths. Model labels are inserted with textContent into static markup.
Changed: `examples/032-wasm-webcm-agent/web/src/stream.ts`, `examples/032-wasm-webcm-agent/web/package.json`, `examples/032-wasm-webcm-agent/web/package-lock.json`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Headless browser fixtures prove raw handler HTML and unsafe links neither execute nor survive, malicious model labels remain literal, and normal bold Markdown/highlighted code remain intact in both finalization paths.

**Independent fix review (fixed):** Both finalized Markdown insertion paths sanitize parsed HTML through DOMPurify. Model labels are text nodes in static markup, not executable interpolated HTML.
- [`examples/032-wasm-webcm-agent/web/src/stream.ts:10-14,47-51,67-74,185-196`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/stream.ts) Fresh/pending Markdown uses the shared sanitizer, while model-tag values use textContent.
- [`examples/032-wasm-webcm-agent/web/package.json:21-23`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/package.json#L21-L23) DOMPurify is a direct runtime dependency rather than an undeclared test-only global.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final artifact: benign handler marker never executes, dangerous attributes/javascript links are absent, ordinary Markdown/highlighting/literal model labels remain; full suite passes.

<a id="e13"></a>

### E13: WebCM source fails TypeScript checking against its locked xterm-pty API

**Severity:** medium. **Verdict:** confirmed. **Examples:** 032.

**Original proof:** examples/032-wasm-webcm-agent/web/node_modules/.bin/tsc --noEmit --strict --target ES2022 --module ESNext --moduleResolution Bundler --lib ES2022,DOM,DOM.Iterable --skipLibCheck examples/032-wasm-webcm-agent/web/src/*.ts exits 2 with five TS2540 errors at webcm-host.ts:50-54. Counterevidence: actual npm run build passes, and a Node TCGETS/TCSETS probe confirms these writes happen to work in JavaScript; this is a type-check failure, not proof of a runtime boot crash.

- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:49-55`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/webcm-host.ts#L49-L55) The code mutates readonly Termios.iflag/cflag/lflag/oflag returned by TCGETS.
- [`examples/032-wasm-webcm-agent/web/package.json:5-12,17`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/package.json) The build script only runs Vite, so these source type errors are never checked; xterm-pty is a declared dependency.

**Independent challenge:** This is a source type-contract failure, not a runtime boot failure. The browser progressed through PTY configuration to the intentionally missing VM import, which is direct counterevidence to treating these readonly writes as an immediate JavaScript exception.

**Accepted correction:** Construct a fresh TermiosConfig from the snapshot, overriding computed flags, instead of mutating readonly fields. Add a small explicit source type-check lane/config for this example.

- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:49-55`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/webcm-host.ts#L49-L55) Mutates five readonly Termios fields.
- `examples/032-wasm-webcm-agent/web/node_modules/xterm-pty/index.d.ts:51-65,211-212` (unversioned audit evidence) TCGETS returns readonly Termios; TCSETS accepts a TermiosConfig.
- [`examples/032-wasm-webcm-agent/web/package.json:5-12`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/package.json#L5-L12) Build is Vite only; no tsc source validation.

**Implementation (fixed):** Builds a fresh TermiosConfig from the readonly snapshot, with equivalent computed flags. Added explicit strict source typecheck and made build depend on it.
Changed: `examples/032-wasm-webcm-agent/web/src/webcm-host.ts`, `examples/032-wasm-webcm-agent/web/tsconfig.json`, `examples/032-wasm-webcm-agent/web/package.json`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/032-wasm-webcm-agent/web run typecheck`: **pass** tsc --noEmit with strict ES2022/Bundler/DOM source config.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Frozen termios input remains unchanged while all output masks are asserted; actual xterm-pty boot path reaches the intentionally intercepted module boundary.
- `npm --prefix examples/032-wasm-webcm-agent/web run build`: **pass** Strict tsc then Vite bundle. Vite emits an advisory large-chunk warning; no check is disabled.

**Independent fix review (fixed):** PTY flag changes construct a fresh TermiosConfig rather than mutating readonly values. The example has an explicit strict TypeScript configuration and build-time checking.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:22-30,96-98`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/webcm-host.ts) Computed flags are projected into a new object and passed to TCSETS.
- [`examples/032-wasm-webcm-agent/web/tsconfig.json:1-15`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/tsconfig.json#L1-L15) Strict/noEmit browser source checking is configured.
- [`examples/032-wasm-webcm-agent/web/package.json:7-9`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/package.json#L7-L9) build executes tsc before Vite; typecheck is available independently.
- `npm --prefix examples/032-wasm-webcm-agent/web run build`: **pass** Strict tsc and Vite production bundle passed. Frozen readonly snapshot/raw flag equivalence also passed in the regression suite.

<a id="e14"></a>

### E14: Retrying a failed WebCM boot accumulates terminal and PTY resources

**Severity:** medium. **Verdict:** partial. **Examples:** 032.

**Original proof:** audit-E-browser.mjs opens the real built app offline, enters a synthetic key, and triggers the missing-webcm.mjs error twice. The DOM contains one .xterm after attempt 1 and two after attempt 2.

- [`examples/032-wasm-webcm-agent/web/src/main.ts:112-120,168-173`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/main.ts) Every retry calls vm.boot again; the catch merely displays the error and re-enables the button.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:26-45,59-79`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/webcm-host.ts) boot creates a new Terminal, PTY, output listener and imported VM instance, overwriting stored references without disposal or a booted guard.

**Independent challenge:** Repeated pre-import boot failures demonstrably accumulate terminal/PTY resources. The submitted claim also requests cleanup of partially initialized mob subscriptions and implies emulator leakage after successful initialization; neither was exercised by the cited failure, so that broader cleanup is not automatically accepted under this ID.

**Accepted correction:** Give WebCMHost a single owned boot lifecycle: dispose allocations/listeners on partial boot failure, prevent concurrent boot, and reuse an already booted host rather than allocating another terminal. Only claim supported emulator disposal after checking its actual API. Mob-subscription cleanup needs separate direct evidence/adjudication.

- [`examples/032-wasm-webcm-agent/web/src/main.ts:112-120,168-173`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/main.ts) Retry re-enters vm.boot; catch only updates status and re-enables the button.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:26-79`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/webcm-host.ts#L26-L79) Allocates Terminal, PTY and listener before the failing import, overwrites references, and has no boot guard or failure disposal.

**Implementation (fixed):** Narrow accepted scope only: WebCMHost shares one in-flight boot, reuses a booted host and disposes terminal/PTY output listener on partial boot failure. No unsupported emulator-destruction API or unadjudicated mob-subscription cleanup was added.
Changed: `examples/032-wasm-webcm-agent/web/src/webcm-host.ts`, `examples/032-wasm-webcm-agent/README.md`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`, `examples/032-wasm-webcm-agent/web/tests/guest-smoke.mjs`.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Three repeated pre-import browser failures leave no terminal or output-listener owner. Concurrent actual boot calls share the same promise. Separate synthetic emulator-module test proves an already-booted host keeps one terminal and invokes module boot once. This is not real emulator disposal coverage.

**Independent fix review (fixed):** Within the independently accepted pre-import-failure scope, one boot promise owns resources, failed boot disposes the output listener and terminal, and successful repeated boot reuses the owner. No unsupported emulator shutdown or broader mob cleanup is claimed.
- [`examples/032-wasm-webcm-agent/web/src/webcm-host.ts:51-73,101-109`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/webcm-host.ts) One in-flight promise coalesces concurrent boots; catch disposes/nulls allocations and clears stale output.
- [`examples/032-wasm-webcm-agent/README.md:113-115`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/README.md#L113-L115) The documentation explicitly bounds the cleanup guarantee.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final artifact: three missing-module retries leave no terminal/listener/slave; concurrent boots share a promise and successful boots reuse one terminal. Full Meerkat startup also passes.

<a id="e15"></a>

### E15: A partial WebCM download is permanently accepted as a complete cache

**Severity:** medium. **Verdict:** confirmed. **Examples:** 032.

**Original proof:** audit-E-probes.mjs executes the launcher's exact download section in a project-local isolated fixture with a fake curl: JS succeeds, WASM fails. First invocation exits 22. Retry exits 0 with 'WebCM already downloaded', while webcm.wasm still does not exist. No network or Rust build is invoked.

- [`examples/032-wasm-webcm-agent/examples.sh:31-41`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/examples.sh#L31-L41) Cache validity checks only existence of webcm.mjs. It writes that file before downloading webcm.wasm, without atomic staging or checking both artifacts.

**Independent challenge:** The cache predicate certifies only one member of the pair. The failed second curl is a normal path under set -e and leaves the first member in place.

**Accepted correction:** Require a complete nonempty pair before cache reuse; use staging/atomic promotion or a retry-safe missing-member repair so a failed download cannot poison subsequent launches.

- [`examples/032-wasm-webcm-agent/examples.sh:31-41`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/examples.sh#L31-L41) webcm.mjs existence bypasses both downloads without checking webcm.wasm or file size.

**Implementation (fixed):** Extracted a sourceable download helper requiring a nonempty JS/WASM pair. Downloads stage in example-local .part files and only promote after both succeed; failure removes staging so retry cannot certify the old JS-only state.
Changed: `examples/032-wasm-webcm-agent/download-webcm.sh`, `examples/032-wasm-webcm-agent/examples.sh`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Real helper and curl against a loopback synthetic HTTP server exercise JS-only, WASM-only, zero-byte, complete cache, second-download failure and subsequent successful retry.
- `bash -n examples/032-wasm-webcm-agent/examples.sh examples/032-wasm-webcm-agent/download-webcm.sh`: **pass** Both shell scripts parse.

**Independent fix review (fixed):** A cache hit requires both nonempty files. Both downloads stage before promotion, and failed downloads remove staging; JS-only/WASM-only/empty caches retry instead of being certified complete.
- [`examples/032-wasm-webcm-agent/download-webcm.sh:3-19`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/download-webcm.sh#L3-L19) Complete-pair predicate, checked downloads, cleanup and promotion implement retry-safe cache population.
- [`examples/032-wasm-webcm-agent/examples.sh:31-32`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/examples.sh#L31-L32) The actual launcher sources and calls the tested helper.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final artifact: loopback cache regression passes partial/zero/complete and failed-WASM-then-retry cases; entire suite passes.
- `bash -n examples/032-wasm-webcm-agent/examples.sh examples/032-wasm-webcm-agent/download-webcm.sh`: **pass** Both launcher/helper parse successfully.

<a id="e16"></a>

### E16: Diplomacy README documents a retired faction and channel UI

**Severity:** low. **Verdict:** confirmed. **Examples:** 031.

**Original proof:** Direct documentation-to-source comparison; the offline Chrome run confirms 9 .grid-cell elements and the current map/UI.

- [`examples/031-wasm-mini-diplomacy-sh/README.md:20-24,39-46,118-124,134-138`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/README.md) The guide names North/South/East factions and N/S/E channels, tells readers to click sidebar channels, and presents main.ts as owning mob definitions/event streaming.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/types.ts:5-7,35-45`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/types.ts) Current factions/channels are France, Prussia, Russia and a correspondent.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/ui.ts:62-69,183-211`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/ui.ts) Current UI is a 3x3 channel grid with per-cell compact/verbose toggles, not a sidebar selector.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:9-13`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L9-L13) Mob definitions and event routing are split into agents.ts and events.ts.

**Independent challenge:** The README's faction names, navigation and file ownership are stale. Its ten conceptual channels are not itself a bug: nine grid channels plus the separate narrator remain ten channels.

**Accepted correction:** Update faction/channel names, actual controls and module ownership. Preserve correct runtime prerequisites and the distinction between nine chat cells and the narrator feed.

- [`examples/031-wasm-mini-diplomacy-sh/README.md:20-24,39-46,118-124,134-138`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/README.md) North/South/East, sidebar navigation and monolithic main.ts do not describe current source/UI.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/ui.ts:62-69,183-211`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/ui.ts) Nine grid cells with per-cell compact/verbose toggles.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:9-13,30-35`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) Split modules and a button labeled Start.

**Implementation (fixed):** Updated France/Prussia/Russia, the nine-cell grid plus separate tenth narrator channel, actual Start/guide/arrow controls, safe-boundary scheduling semantics and actual module ownership. Preserved runtime prerequisites and in-memory limitations.
Changed: `examples/031-wasm-mini-diplomacy-sh/README.md`.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Full actual UI smoke covers Start, guide dismissal, Pause, Step, map turn display and narrator; revised file table compared directly with imports/modules.
- `git diff --check -- examples/031-wasm-mini-diplomacy-sh`: **pass** No whitespace errors.

**Independent fix review (fixed):** README now matches the actual France/Prussia/Russia grid, separate narrator feed, overlays, controls and module ownership. Runtime limitations remain explicit rather than representing mock checks as a campaign.
- [`examples/031-wasm-mini-diplomacy-sh/README.md:23-46,125-165`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/README.md) Correct roster, nine chat cells plus narrator, guide/control behavior and actual file responsibilities.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:25-109,480-496`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) Visible labels and control wiring match the rewritten instructions.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web run build`: **pass** Current documented module split typechecks and bundles.
- `git diff --check -- examples/031-wasm-mini-diplomacy-sh examples/032-wasm-webcm-agent examples/033-the-office-demo-sh`: **pass** No whitespace errors. README/source cross-check completed.

<a id="e17"></a>

### E17: Narrator battle summaries attribute every changed territory to every attacker

**Severity:** medium. **Verdict:** confirmed. **Examples:** 031.

**Original proof:** audit-E-probes.mjs fixes Math.random to zero, has France attack Bavaria with aggression 0 (repelled) and Russia attack Bavaria with aggression 100 (captures), then runs the actual summary builder. It reports both 'France ... CAPTURED' and 'Russia ... CAPTURED'.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:128-133`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts#L128-L133) Each order is labeled CAPTURED solely by captures.has(target_region), not whether that attacker won.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:269-274,282-284`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) captures is only the set of regions whose final owner differs from the pre-turn owner.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/game.ts:28-46`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/game.ts#L28-L46) Orders resolve sequentially, so one attack may fail and a later faction can capture the same target.

**Independent challenge:** The changed-region set carries final ownership change, not per-attacker success. This loses intermediate battle facts and falsely marks a failed attacker as successful when another later attacker captures the target.

**Accepted correction:** Return per-order outcomes from the combat resolver and feed those facts to narration. Keep the final changed-region set for map effects if useful; do not use it to infer individual attack success.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:128-133`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts#L128-L133) CAPTURED depends only on target membership in captures.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/game.ts:28-46`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/game.ts#L28-L46) Orders resolve sequentially with independently observable success/failure.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:269-274,282-284`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) captures contains only final owner differences.

**Implementation (fixed):** Combat returns per-order captured/repelled/skipped facts alongside the resulting state. Narration uses those facts; final changed-region set remains only for map effects and final territory summary.
Changed: `examples/031-wasm-mini-diplomacy-sh/web/src/game.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/types.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/main.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/events.ts`, `examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs`.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Actual deterministic resolver and narrator cover failure-then-capture, two captures, recapture to original owner, already-owned target and nonexistent target. Each narration line matches its order, irrespective of final ownership.

**Independent fix review (fixed):** Combat now returns each order's own captured/repelled/skipped fact, and narration consumes it. Final ownership change is used only for the aggregate territory/map summary.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/game.ts:29-61`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/game.ts#L29-L61) Each order records its actual outcome at the moment combat resolves, preserving intermediate captures/recaptures.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:142-154`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts#L142-L154) Narration prints the outcome fact rather than inferring attacker success from a changed-region set.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Final artifact: repelled-then-captured, repeated captures, recapture to original owner, skipped targets and full startup pass.

<a id="e18"></a>

### E18: Diplomacy deduplicates provider-local tool IDs across the entire campaign

**Severity:** high. **Verdict:** partial. **Examples:** 031.

**Original proof:** audit-E-probes.mjs feeds two different member subscriptions with send_message id='fc_0'. Only the first message is retained. The fixture includes the legacy to field to isolate ID suppression from the separate canonical-addressing defect E19.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:66-70`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts#L66-L70) Every member and turn shares session.seenToolCallIds; an id seen anywhere is dropped before routing.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:402-403`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L402-L403) The set is created once per campaign, not per member/run.
- [`meerkat-gemini/src/client.rs:2167,2233-2234`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-gemini/src/client.rs) Each Gemini response stream starts tool_call_index at zero and assigns fc_0, fc_1, etc. These are not globally unique event identities.

**Independent challenge:** Campaign-global provider-call-ID deduplication is incorrect, but the existing 031 ingress currently rejects genuine envelopes first. The original reproduction also depends on legacy to addressing to get past E19. Thus this is a confirmed downstream collision bug, not an independently reachable current-runtime end-to-end reproduction.

**Accepted correction:** Deduplicate actual envelope identity, preferably event_id, rather than provider-local call IDs. Maintain independent member/response/turn events even when call IDs repeat. Do not fold the separately discovered ingress mismatch into E18.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:24-36,66-79`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts) Envelope gate precedes one campaign-wide seenToolCallIds set and legacy addressing.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:402-403`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L402-L403) The set is created once per campaign.
- [`meerkat-gemini/src/client.rs:2167,2233-2234`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-gemini/src/client.rs) Each provider response resets the counter and emits fc_0, fc_1.
- [`meerkat-core/src/agent/state.rs:6040-6069`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L6040-L6069) The core forwards the provider call ID unchanged to ToolCallRequested.

**Implementation (fixed):** Replaced campaign-global provider call-ID deduplication with canonical envelope event_id replay suppression. Independent events retain subscription attribution even when provider-local IDs repeat.
Changed: `examples/031-wasm-mini-diplomacy-sh/web/src/events.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/types.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/main.ts`, `examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs`.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Current envelopes from distinct members, responses and turns sharing one provider call ID all render; only replay of the identical event is suppressed. Full UI mock reuses IDs again on a second stepped round.

**Independent fix review (fixed):** Provider-local tool call IDs no longer control diplomacy event suppression. Canonical event_id replay detection retains independent member and later-turn events with identical provider IDs.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:70-73,86-97`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts) Replay guard uses envelope event_id, and routing attribution comes from the associated subscription.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:413-415`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L413-L415) A fresh campaign gets a fresh replay set.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Final artifact: provider IDs across members/turns remain distinct, true envelope replay is suppressed, and full startup passes.

<a id="e19"></a>

### E19: Peer messaging prompts and diplomacy routing still use retired name-addressed sends

**Severity:** high. **Verdict:** partial. **Examples:** 031, 032.

**Original proof:** Current SendMessageInput requires peer_id, so the literal legacy calls prescribed in the skills fail at argument deserialization. Separately, audit-E-probes.mjs supplies a valid current peer_id/display_name/body send event and 031 records zero messages. This finding does not claim every LLM will obey stale prompts rather than correct itself from tool schemas; the deterministic event-loss branch is unconditional for a canonical call without to.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts:32-46,52-66,74-89`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts) Role skills demand sending to fixed display addresses and explicitly prohibit peers(), which is now the canonical identity-discovery tool.
- [`examples/032-wasm-webcm-agent/web/src/mob.ts:145-162,181-184,197-201,215-218`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/mob.ts) Role skills demonstrate send_message(to: 'dev-team/...', ...) using a removed addressing field.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:71-79`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts#L71-L79) DM routing reads only args.to. A legitimate current send_message event has peer_id and optional display_name instead.
- [`meerkat-comms/src/mcp/tools.rs:64-83,303,360-363`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-comms/src/mcp/tools.rs) SendMessageInput requires peer_id; peers returns canonical peer_id/name descriptors; dispatch deserialization rejects a legacy call without peer_id.
- [`meerkat-comms/src/agent/dispatcher.rs:1-6,93-97`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-comms/src/agent/dispatcher.rs) The agent-facing comms dispatcher uses the same tools/schema path, so this is not merely an unrelated MCP-only contract.

**Independent challenge:** The prompts really teach obsolete addressing and 031's payload router reads only to. However, the audit overstates its current-event reproduction: actual envelopes are first rejected by E-NEW-01. An LLM may repair bad prompt examples by following the current tool schema, so universal agent messaging failure is not proven; the invalid instructions and downstream routing mismatch are.

**Accepted correction:** Teach discovery via peers and canonical peer_id sends (or runtime-bound reply_to_peer). Populate host identity-to-member routing from actual peer descriptors, not optional labels. Update extraction prompts that still prescribe display addresses. Keep E-NEW-01 separate.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts:32-46,52-66,74-89`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts) Demands display-name addresses and explicitly forbids peers discovery.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:71-79`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts#L71-L79) Only args.to selects the DM channel.
- [`examples/032-wasm-webcm-agent/web/src/mob.ts:145-162,181-184,197-201,215-218`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/032-wasm-webcm-agent/web/src/mob.ts) Demonstrates send_message(to:...) calls.
- [`meerkat-comms/src/mcp/tools.rs:64-83,301-304,360-363`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-comms/src/mcp/tools.rs) SendMessageInput requires UUID PeerId; peers discovers canonical identities and serde rejects missing peer_id.
- [`meerkat-web-runtime/src/lib.rs:2229-2295`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L2229-L2295) mob_member_peer_target resolves actual identity-bearing trusted peer descriptors usable for host routing.

**Implementation (fixed):** All inline skills teach peers discovery and send_message(peer_id, handling_mode: queue, body), not legacy to/display addresses. Diplomacy binds returned external.peer_id descriptors to the requested member identity and routes messages/structured dispatches through that map; extraction prompts carry canonical planner peer IDs.
Changed: `examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/events.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/main.ts`, `examples/031-wasm-mini-diplomacy-sh/web/src/types.ts`, `examples/032-wasm-webcm-agent/web/src/mob.ts`, `examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs`, `examples/032-wasm-webcm-agent/web/tests/regression.test.mjs`.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Canonical intra-/cross-mob current-envelope routing does not require to or display labels. Full UI receives deliberately misleading descriptor labels and still routes FINAL ORDER by peer_id. Actual WASM descriptor shape and all faction/narrator definitions verified offline.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Every emitted inline skill is checked for canonical addressing plus required handling_mode; all provider-assignment definitions accepted by actual current WASM. Compared instructions with actual SendMessageInput schema in meerkat-comms/src/mcp/tools.rs.

**Independent fix review (fixed):** Both examples' prompts now require peers discovery and canonical peer_id addressing. Diplomacy maps actual member targets to identities and uses that map for raw and structured routing, independent of display labels.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts:28-39,115-117`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/agents.ts) Shared routing instruction and structured dispatch schema require discovered canonical IDs.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:218-229,378-382`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts) Extraction prompts and map construction use actual external.peer_id values.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:90-97,111-116`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts) Neither raw nor structured routing guesses from display names.
- [`examples/032-wasm-webcm-agent/web/src/mob.ts:145-160,187-224`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/032-wasm-webcm-agent/web/src/mob.ts) All role prompts teach peers discovery and queued canonical sends, not retired to addresses.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Final artifact: canonical routing/prompt/peer-target tests and all actual intra/cross-mob wiring pass; successful structured member events observed.
- `npm --prefix examples/032-wasm-webcm-agent/web test`: **pass** Final artifact: four emitted definitions, prompt contracts, actual six-wire startup and all member typed events pass. No real-provider autonomous peer conversation is claimed.

<a id="e-new-01"></a>

### E-NEW-01: Diplomacy rejects every current member event because it requires retired source_id

**Severity:** high. **Verdict:** confirmed. **Examples:** 031.

**Original proof:** Real current WASM: created one autonomous member with a synthetic Anthropic key and intercepted local 401 provider response. mob_member_subscribe/poll_subscription returned one run_failed envelope whose keys were event_id,payload,seq,source,timestamp_ms; source was {type:'session',session_id:<UUID>}; no source_id existed. Passing that shape to the actual drainAllEvents returned events=0/errors=[] and emitted Skipping malformed event envelope. Payload-specific fixes alone cannot restore the example.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:24-36,44-52`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts) Requires top-level event.source_id to be a string, then sorts on it.
- [`meerkat-core/src/event.rs:27-39,91-105`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs) EventEnvelope has typed source, not a top-level source_id string.
- [`meerkat-web-runtime/src/lib.rs:2894-2921`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs#L2894-L2921) Member polling serializes the canonical EventEnvelope directly; no source_id compatibility projection is added.
- [`sdks/web/src/mob.ts:547-598`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/sdks/web/src/mob.ts#L547-L598) The maintained SDK validates the typed source variants.

**Independent challenge:** The example requires a top-level source_id string before considering any event payload. The current canonical EventEnvelope instead serializes a typed source field, with source_id occurring only inside the External source variant. The example subscribes through mob_member_subscribe, whose raw per-session envelopes are serialized without a compatibility projection. I independently loaded the existing 0.8.40 WASM, produced a real member run_failed envelope with an entirely intercepted synthetic HTTP response, and fed those exact runtime bytes into the actual transpiled drainAllEvents function. It returned events=0, errors=[] and one malformed-envelope warning. Adding only a legacy top-level source_id to a test-only copy made events=1. This isolates the ingress rejection independently of any other payload drift.

**Accepted correction:** Update example031's envelope ingress and ordering to consume the canonical typed source identity. Keep attribution from the subscription's agentIdentity/role/team rather than deriving member identity from a source string. Use a deterministic discriminant-plus-identifier comparison for typed sources alongside timestamp, sequence and event ID, and reject genuinely malformed canonical sources. Do not add a shadow top-level source_id to core/WASM or mutate runtime envelopes to satisfy the legacy example.

- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:16-39,44-58`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts) The guard requires typeof event.source_id === 'string'; otherwise it warns once and continues without buffering. Sorting also depends on the retired field.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:371-373`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L371-L373) Each faction member subscription is created with mob_member_subscribe, so the relevant transport branch is Member, not the structurally different mob-wide AttributedEvent branch.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/main.ts:158-171`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/031-wasm-mini-diplomacy-sh/web/src/main.ts#L158-L171) The returned event count refreshes lastEventTime and returned errors populate allErrors. Discarding all current envelopes therefore affects liveness observation and event-driven processing, not just cosmetic logging.
- [`meerkat-core/src/event.rs:25-39,91-105,978-988`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/event.rs) EventEnvelope derives Serialize and has source: EventSourceIdentity, not a top-level source_id. Session envelopes use the Session variant. External.source_id is nested and cannot satisfy the example's check.
- [`meerkat-session/src/ephemeral.rs:6707-6716`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/ephemeral.rs#L6707-L6716) The session task fixes its canonical source to EventSourceIdentity::session(session_id).
- [`meerkat-web-runtime/src/lib.rs:2821-2858,2890-2933,2967-2975`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-web-runtime/src/lib.rs) mob_member_subscribe obtains the raw session-event receiver. poll_subscription serializes each received event through serde_json::to_value, then serializes the array. No top-level source_id is synthesized.
- [`sdks/web/src/mob.ts:547-598`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/sdks/web/src/mob.ts#L547-L598) The maintained SDK independently validates the canonical discriminated source variants, corroborating the current wire contract rather than a legacy string projection.
- `sdks/web/wasm/meerkat_web_runtime.js:existing built artifact` (unversioned audit evidence) Executed with its paired meerkat_web_runtime_bg.wasm via initSync; runtime_version() returned 0.8.40. Actual polled member envelope keys were event_id,payload,seq,source,timestamp_ms; source was {type:'session',session_id:'21ccf0ab-6fcf-4933-8922-c71ee925ba3c'} and payload.type was run_failed.

**Implementation (fixed):** Implemented only after reading the independent confirmed verdict in challenge-E-addendum.json. Envelope ingress validates canonical typed source variants and ordering uses discriminant-plus-identifier, timestamp, sequence and event ID. Attribution still comes from the member subscription. No SDK/runtime shadow source_id was added.
Changed: `examples/031-wasm-mini-diplomacy-sh/web/src/events.ts`, `examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs`.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Real 0.8.40 WASM member run_failed bytes pass directly from poll_subscription to actual drainAllEvents with >0 events, diagnostic and zero malformed warnings. fetch is replaced with an in-memory synthetic 401 Response, never forwarded. Also verifies distinct session IDs, timestamp/sequence/event-ID ties, all canonical source variants and malformed-source rejection.

**Independent fix review (fixed):** The independently challenged envelope-ingress mismatch is corrected without modifying core envelopes. Typed source variants are validated/sorted canonically; current raw member envelopes reach the actual drain unchanged.
- [`examples/031-wasm-mini-diplomacy-sh/web/src/events.ts:11-20,37-69`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/src/events.ts) No top-level source_id requirement remains; sort order is timestamp/source/sequence/event ID.
- [`meerkat-core/src/event.rs:25-39,91-105`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-core/src/event.rs) The canonical type-tagged source and envelope fields align with the new guard.
- [`examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs:270-329`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/031-wasm-mini-diplomacy-sh/web/tests/regression.test.mjs#L270-L329) Actual WASM member envelopes, not compatibility-enriched copies, feed drainAllEvents and yield a typed failure with no malformed-envelope warning.
- `npm --prefix examples/031-wasm-mini-diplomacy-sh/web test`: **pass** Final artifact: unchanged real-WASM ingress, equal-time source/sequence/ID ordering, malformed-source fixtures and full startup all pass.
