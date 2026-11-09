import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

const bundled = await build({ entryPoints: [fileURLToPath(new URL("rehearsal-panel.mjs", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent" });
const { rehearsalCurrent, rehearsalConversation, rehearsalActivity, rehearsalActivityFromTimeline, rehearsalReplay,
  applicationTrialAvailable, applicationTrialMatches, applicationTrialFixtures, applicationTrialSamples, applicationTrialRepair,
  applicationTrialFeedback, applicationTrialImprovement, rehearsalNavigationIntent, rehearsalNavigationMode,
  rehearsalRunRequest, applicationTrialSummary, createRehearsalReceiptEpoch, rehearsalActive, rehearsalCanContinue,
  rehearsalContinueRequest, admitRehearsalFollowUp, rehearsalSessionSummary, rehearsalSamplePreview,
  applicationTrialPreview, applicationTrialEndToRepair } = await import(
  `data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);
const state = { id: "reading", version: 7 };
const receipt = { rehearsal_id: "trial", guide_id: "reading", guide_version: 7, current: true, status: "running",
  identities: [{ identity: "lead:main", name: "Reading companion" }, { identity: "librarian:main", name: "Librarian" }] };
const native = (id, kind, identity, interaction, payload = {}, extra = {}) => ({ id, kind, identity,
  interaction_id: interaction, payload, cursor: `console:${id}`, frame_version: 1, timestamp_ms: Number(id) * 1000,
  source: { kind: "runtime" }, ...extra });

function delayedResponse() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

test("a GET started before local admission cannot replace its newer receipt or connection state", async () => {
  const epoch = createRehearsalReceiptEpoch(state.id), delayed = delayedResponse();
  let displayed = null, unavailable = true;
  const callbacks = { onReceipt: (value) => { displayed = value; }, onUnavailable: (value) => { unavailable = value; } };
  const pending = epoch.read(() => delayed.promise, () => state, callbacks);
  // The same epoch boundary is used when the production POST admits a trial.
  epoch.invalidate(); displayed = { ...receipt, rehearsal_id: "new-admission", status: "queued" }; unavailable = false;
  delayed.resolve({ ...receipt, rehearsal_id: "older-trial", status: "completed" });
  assert.equal(await pending, false);
  assert.equal(displayed.rehearsal_id, "new-admission");
  assert.equal(displayed.status, "queued");
  assert.equal(unavailable, false);

  const duringAdmission = delayedResponse();
  const second = epoch.read(() => duringAdmission.promise, () => state, callbacks);
  epoch.invalidate(); displayed = { ...displayed, status: "starting" };
  duringAdmission.reject(Object.assign(new Error("Old connection failure"), { status: 503 }));
  assert.equal(await second, false);
  assert.equal(displayed.status, "starting");
  assert.equal(unavailable, false);
});

test("fresh polls retain same-trial progress and genuine server-selected history without inferred receipt ordering", async () => {
  const epoch = createRehearsalReceiptEpoch(state.id), values = [], availability = [];
  const callbacks = { onReceipt: (value) => values.push(value), onUnavailable: (value) => availability.push(value) };
  for (const value of [{ ...receipt, status: "starting" }, { ...receipt, status: "running" },
    { ...receipt, status: "completed" }, { ...receipt, rehearsal_id: "server-new-trial", status: "queued" },
    { ...receipt, current: false, guide_version: state.version - 1, status: "failed" }]) {
    assert.equal(await epoch.read(async () => value, () => state, callbacks), true);
    assert.equal(values.at(-1), value);
  }
  assert.deepEqual(availability, [false, false, false, false, false]);
  assert.equal(values.at(-1).current, false);
  assert.equal(await epoch.read(async () => { throw Object.assign(new Error("No saved trial"), { status: 404 }); },
    () => state, callbacks), true);
  assert.equal(availability.at(-1), false);
  assert.equal(values.length, 5);
});

test("guide changes and unmount suppress delayed poll results and pending action ownership", async () => {
  for (const changed of [{ ...state, id: "other-guide" }, { ...state, version: state.version + 1 }, null]) {
    const epoch = createRehearsalReceiptEpoch(state.id), delayed = delayedResponse();
    let guide = state, updates = 0;
    const action = epoch.capture(guide);
    const pending = epoch.read(() => delayed.promise, () => guide,
      { onReceipt: () => { updates += 1; }, onUnavailable: () => { updates += 1; } });
    if (changed) guide = changed;
    else epoch.stop();
    delayed.resolve(receipt);
    assert.equal(await pending, false);
    assert.equal(epoch.current(action, guide), false);
    assert.equal(updates, 0);
    if (changed?.id !== state.id) assert.equal(epoch.capture(changed), null);
  }
});

test("cancel boundaries cannot resurrect running receipts and stopped readers cannot restart", async () => {
  const epoch = createRehearsalReceiptEpoch(state.id), delayed = delayedResponse();
  let displayed = receipt, unavailable = false;
  const callbacks = { onReceipt: (value) => { displayed = value; }, onUnavailable: (value) => { unavailable = value; } };
  const pending = epoch.read(() => delayed.promise, () => state, callbacks);
  epoch.invalidate();
  const cancel = epoch.capture(state);
  assert.equal(epoch.current(cancel, state), true);
  epoch.invalidate(); displayed = { ...receipt, status: "interrupted" };
  delayed.resolve(receipt);
  assert.equal(await pending, false);
  assert.equal(displayed.status, "interrupted");
  assert.equal(epoch.current(cancel, state), false);
  const settled = { ...displayed, detail: "Actual server cancellation result" };
  assert.equal(await epoch.read(async () => settled, () => state, callbacks), true);
  assert.equal(displayed, settled);
  epoch.stop();
  assert.equal(await epoch.read(async () => { throw new Error("Must not load after unmount"); }, () => state, callbacks), false);
  assert.equal(displayed, settled);
  assert.equal(unavailable, false);
});

test("actual native peer text is attributed to its known identity and stale trials cannot drive the proposal", () => {
  const events = [native("1", "text_delta", "lead:main", "first", { delta: "I will ask the librarian." }),
    native("2", "text_delta", "librarian:main", "peer", { delta: "A live collection adapter is unavailable in this trial." })];
  const frames = rehearsalReplay(events, 1);
  const entries = rehearsalConversation(receipt, frames).entries;
  assert.ok(entries.filter((entry) => entry.identity.role === "assistant").every((entry) => entry.identity.showLabel));
  const rendered = JSON.stringify(entries);
  assert.ok(rendered.includes("Reading companion"));
  assert.ok(rendered.includes("Librarian"));
  assert.ok(rendered.includes("unavailable in this trial"));
  assert.deepEqual(rehearsalActivity(receipt, frames, state).activeNodeIds, ["lead:main", "librarian:main"]);
  for (const changed of [{ current: false }, { guide_id: "other" }, { guide_version: 6 }]) {
    assert.equal(rehearsalCurrent({ ...receipt, ...changed }, state), false);
    assert.equal(rehearsalActivity({ ...receipt, ...changed }, frames, state, { replay: true }), null);
  }
});

test("completing one native run does not make another concurrent run on the same agent disappear", () => {
  const events = [native("1", "run_started", "lead:main", "first", {}, { run_id: "run-1" }),
    native("2", "run_started", "lead:main", "second", {}, { run_id: "run-2" }),
    native("3", "interaction_complete", "lead:main", "first", { source_event_type: "run_completed" })];
  assert.deepEqual(rehearsalActivity(receipt, rehearsalReplay(events, 2), state).activeNodeIds, ["lead:main"]);
  events.push(native("4", "interaction_complete", "lead:main", "second", { source_event_type: "run_completed" }));
  assert.deepEqual(rehearsalActivity(receipt, rehearsalReplay(events, 3), state).activeNodeIds, []);
});

test("trace replay preserves the tool state that was observed at that point, including native version updates", () => {
  const tool = native("2", "tool_call_requested", "librarian:main", "peer", { name: "read_file", tool_call_id: "call-1" });
  const events = [native("1", "run_started", "librarian:main", "peer", {}, { run_id: "peer-run" }), tool,
    native("3", "frame_updated", "librarian:main", "peer", { frame: { ...tool, frame_version: 2,
      status: "completed", payload: { ...tool.payload, result: "A sample title", status: "completed" } } }),
    native("4", "interaction_complete", "librarian:main", "peer", { source_event_type: "run_completed" })];
  const before = rehearsalReplay(events, 1), after = rehearsalReplay(events, 2);
  assert.equal(before.find((frame) => frame.id === "2").frameVersion, 1);
  assert.equal(after.find((frame) => frame.id === "2").frameVersion, 2);
  assert.equal(after.find((frame) => frame.id === "2").data.result, "A sample title");
  assert.deepEqual(rehearsalActivity({ ...receipt, status: "completed" }, before, state).activeNodeIds, []);
  assert.deepEqual(rehearsalActivity({ ...receipt, status: "completed" }, before, state, { replay: true }).activeNodeIds, ["librarian:main"]);
  assert.deepEqual(rehearsalActivity({ ...receipt, status: "completed" }, rehearsalReplay(events, 3), state,
    { replay: true }).activeNodeIds, []);
});

test("unrecognized identities and an empty trace never create activity in the current proposal", () => {
  assert.equal(rehearsalActivity(receipt, [], state), null);
  const frames = rehearsalReplay([native("1", "text_delta", "invented:main", "other", { delta: "success" })], 0);
  assert.deepEqual(rehearsalActivity(receipt, frames, state).activeNodeIds, []);
});

test("a later native peer continuation without run IDs remains active after an earlier run completed", () => {
  const events = [native("1", "run_started", "lead:main", undefined),
    native("2", "interaction_complete", "lead:main", undefined, { source_event_type: "run_completed" }),
    native("3", "run_started", "lead:main", undefined), native("4", "turn_started", "lead:main", undefined)];
  assert.deepEqual(rehearsalActivity(receipt, rehearsalReplay(events, 1), state).activeNodeIds, []);
  assert.deepEqual(rehearsalActivity(receipt, rehearsalReplay(events, 3), state).activeNodeIds, ["lead:main"]);
  events.push(native("5", "interaction_complete", "lead:main", undefined, { source_event_type: "run_completed" }));
  assert.deepEqual(rehearsalActivity(receipt, rehearsalReplay(events, 4), state).activeNodeIds, []);
});

test("replayed source-event starts and terminals cannot resurrect old runs or clear a newer no-ID continuation", () => {
  const first = native("1", "run_started", "lead:main", undefined, {}, { source_event_id: "old-start" });
  const ended = native("2", "interaction_complete", "lead:main", undefined,
    { source_event_type: "run_completed" }, { source_event_id: "old-terminal" });
  const second = native("3", "run_started", "lead:main", undefined, {}, { source_event_id: "new-start" });
  const events = [first, ended, second,
    { ...first, id: "4", frame_version: 2 }, { ...ended, id: "5", frame_version: 2 }];
  assert.deepEqual(rehearsalActivityFromTimeline(receipt, events.slice(0, 2), state).activeNodeIds, []);
  assert.deepEqual(rehearsalActivityFromTimeline(receipt, events, state).activeNodeIds, ["lead:main"]);
  events.push(native("6", "run_completed", "lead:main", undefined, {}, { source_event_id: "new-terminal" }));
  events.push({ ...second, id: "7", frame_version: 3 });
  assert.deepEqual(rehearsalActivityFromTimeline(receipt, events, state).activeNodeIds, []);
});

test("stable actual frame IDs dedupe nested updates when native source-event IDs are absent", () => {
  const first = native("1", "run_started", "librarian:main", undefined);
  const ended = native("2", "run_completed", "librarian:main", undefined);
  const second = native("3", "run_started", "librarian:main", undefined);
  const updated = (id, frame) => native(id, "frame_updated", "librarian:main", undefined,
    { frame: { ...frame, frame_version: 2 } });
  const events = [first, ended, second, updated("4", first), updated("5", ended)];
  assert.deepEqual(rehearsalActivityFromTimeline(receipt, events, state).activeNodeIds, ["librarian:main"]);
  events.push(native("6", "run_cancelled", "librarian:main", undefined));
  events.push(updated("7", second));
  assert.deepEqual(rehearsalActivityFromTimeline(receipt, events, state).activeNodeIds, []);
});

test("an explicitly bound terminal cannot clear a separate active run on the same identity", () => {
  const events = [native("1", "run_started", "lead:main", undefined, {}, { run_id: "first" }),
    native("2", "run_started", "lead:main", undefined, {}, { run_id: "second" }),
    native("3", "run_failed", "lead:main", undefined, {}, { run_id: "first" })];
  assert.deepEqual(rehearsalActivityFromTimeline(receipt, events, state).activeNodeIds, ["lead:main"]);
  events.push(native("4", "run_completed", "lead:main", undefined, {}, { run_id: "second" }));
  assert.deepEqual(rehearsalActivityFromTimeline(receipt, events, state).activeNodeIds, []);
});

test("a projected interaction completion closes its own streamed activity when only the terminal carries the run ID", () => {
  const events = [native("1", "run_started", "lead:main", "first", {}, { run_id: "first-run" }),
    native("2", "text_delta", "lead:main", "first", { delta: "Finished this request." }),
    native("3", "interaction_complete", "lead:main", "first", { source_event_type: "run_completed" }, { run_id: "first-run" })];
  assert.deepEqual(rehearsalActivityFromTimeline(receipt, events, state).activeNodeIds, []);
  events.push(native("4", "frame_updated", "lead:main", "first", { frame: { ...events[1], frame_version: 2 } }));
  assert.deepEqual(rehearsalActivityFromTimeline(receipt, events, state).activeNodeIds, []);
});

test("application trials require current terminal checked source without pretending independent acceptance", () => {
  const build = { guide_id: state.id, guide_version: state.version, current: true, status: "failed",
    build_id: "checked-build", source_verified: true, source_digest: "source", acceptance: { status: "needs_repair" } };
  assert.equal(applicationTrialAvailable(build, state), true);
  for (const changes of [{ source_verified: false }, { current: false }, { status: "checking" },
    { guide_version: 6 }, { source_digest: null }, { build_id: null }]) assert.equal(applicationTrialAvailable({ ...build, ...changes }, state), false);
  assert.deepEqual(applicationTrialFixtures('{"weather":{"cloud":80}}'), { weather: { cloud: 80 } });
  assert.deepEqual(applicationTrialFixtures(""), {});
  for (const invalid of ["[]", "null", "bad-json", '{"x":1e999}', '{"list":[{"x":-1e999}]}', JSON.stringify({ value: "x".repeat(100001) })])
    assert.throws(() => applicationTrialFixtures(invalid));
  const activity = rehearsalActivityFromTimeline({ ...receipt, mode: "application" },
    [native("1", "run_started", "lead:main", "first")], state);
  assert.equal(activity.scope, "native-application-trial");
});

test("pending builder cleanup blocks a new trial without invalidating an existing source-bound conversation or draft", () => {
  const build = { guide_id: state.id, guide_version: state.version, current: true, status: "completed",
    build_id: "checked-build", source_verified: true, source_digest: "source", cleanup_pending: true,
    acceptance: { status: "pass" } };
  const admitted = { ...receipt, mode: "application", status: "waiting_input", keep_open: true,
    can_continue: true, turn_count: 1, application_source: { build_id: build.build_id, source_digest: build.source_digest } };
  const draft = { mode: "application", sample: "  Compare my saved cases  ", context: "Unsent context\nSecond line", applicationData: '{"cases":["A"]}' };
  const before = structuredClone({ build, admitted, draft });
  assert.equal(applicationTrialAvailable(build, state), false);
  assert.equal(rehearsalNavigationIntent("application", state, build), null);
  assert.throws(() => rehearsalRunRequest(state, build, draft), /build and its runtime cleanup/);
  assert.equal(applicationTrialMatches(admitted, build, state), true);
  assert.equal(rehearsalCanContinue(admitted, build, state), true);
  assert.deepEqual(rehearsalContinueRequest(admitted, build, state, "Continue this sample"), {
    version: state.version, rehearsal_id: admitted.rehearsal_id, expected_turn: 1, text: "Continue this sample" });
  assert.deepEqual({ build, admitted, draft }, before);

  const settled = { ...build, cleanup_pending: false };
  assert.equal(applicationTrialAvailable(settled, state), true);
  const request = rehearsalRunRequest(state, settled, draft);
  assert.equal(request.build_id, build.build_id);
  assert.equal(request.fixtures.files["sample-context.txt"], draft.context);
  assert.deepEqual(draft, before.draft);
});

test("application navigation leads to the actual application wire request without replacing sample drafts", () => {
  const build = { guide_id: state.id, guide_version: state.version, current: true, status: "completed",
    build_id: "checked-build", source_verified: true, source_digest: "source" };
  const draft = { mode: "design", sample: "  Compare these two sample cases  ", context: "My pasted document\nExact second line",
    applicationData: '{"cases":["A","B"]}', flow: "" };
  const original = structuredClone(draft);
  const intent = rehearsalNavigationIntent("application", state, build);
  assert.deepEqual(intent, { guide_id: state.id, guide_version: state.version, mode: "application",
    build_id: "checked-build", source_digest: "source" });
  const mode = rehearsalNavigationMode(intent, state, build, null);
  const body = rehearsalRunRequest(state, build, { ...draft, mode });
  assert.equal(mode, "application");
  assert.deepEqual(body, { version: state.version, mode: "application", build_id: "checked-build", keep_open: true,
    text: "Compare these two sample cases", application_fixtures: { cases: ["A", "B"] },
    fixtures: { files: { "sample-context.txt": draft.context }, notes: [] } });
  assert.deepEqual(draft, original);
  const design = rehearsalRunRequest(state, null, { ...draft,
    mode: rehearsalNavigationMode(rehearsalNavigationIntent("design", state, null), state, null, null), flow: "compare" });
  assert.equal(design.mode, "design");
  assert.equal(Object.hasOwn(design, "build_id"), false);
  assert.equal(Object.hasOwn(design, "application_fixtures"), false);
  assert.deepEqual(design.params, { request: draft.sample.trim() });
  assert.equal(design.keep_open, false);
});

test("trial navigation and submission reject changed guide or source rather than silently trying the design", () => {
  const build = { guide_id: state.id, guide_version: state.version, current: true, status: "failed",
    build_id: "checked-build", source_verified: true, source_digest: "source" };
  const intent = rehearsalNavigationIntent("application", state, build);
  for (const changed of [{ ...intent, guide_id: "other" }, { ...intent, guide_version: 6 },
    { ...intent, guide_version: "7" }, { ...intent, guide_version: true }, { ...intent, mode: "unknown" },
    { ...intent, build_id: "old" }, { ...intent, source_digest: "old" }])
    assert.equal(rehearsalNavigationMode(changed, state, build, null), null);
  for (const changed of [{ ...build, build_id: "new" }, { ...build, source_digest: "new" },
    { ...build, current: false }, { ...build, guide_id: "other" }])
    assert.equal(rehearsalNavigationMode(intent, state, changed, null), null);
  for (const status of ["queued", "starting", "running", "canceling"])
    assert.equal(rehearsalNavigationMode(intent, state, build, { ...receipt, status }), null);
  for (const changed of [{ ...build, current: false }, { ...build, source_verified: false },
    { ...build, status: "running" }, { ...build, build_id: null }]) {
    assert.equal(rehearsalNavigationIntent("application", state, changed), null);
    assert.throws(() => rehearsalRunRequest(state, changed, { mode: "application", sample: "My sample job" }), /build changed/);
  }
  assert.equal(rehearsalNavigationIntent("application", { ...state, version: true }, build), null);
  assert.throws(() => rehearsalRunRequest(state, build, { mode: "application", sample: "My sample job",
    applicationData: "invalid JSON" }));
});

test("trial input mirrors UTF8 fixture bytes and Python ASCII JSON flow bounds without changing drafts", () => {
  const original = { mode: "design", sample: "界".repeat(5000), context: "界".repeat(80_000), flow: "" };
  const before = structuredClone(original);
  assert.throws(() => rehearsalRunRequest(state, null, original), /200 KB UTF8/);
  assert.deepEqual(original, before);
  assert.throws(() => rehearsalRunRequest(state, null, { ...original, context: "", flow: "compare" }), /30 KB parameter/);
  for (const sample of ["界".repeat(4997), "📚".repeat(2498)])
    assert.equal(rehearsalRunRequest(state, null, { mode: "design", sample, flow: "compare" }).params.request, sample);
  for (const sample of ["界".repeat(4998), "📚".repeat(2499)])
    assert.throws(() => rehearsalRunRequest(state, null, { mode: "design", sample, flow: "compare" }), /30 KB parameter/);
  const context = "界".repeat(66_666) + "ab";
  assert.equal(new TextEncoder().encode(context).byteLength, 200_000);
  assert.equal(rehearsalRunRequest(state, null, { mode: "design", sample: "Try", context }).fixtures.files["sample-context.txt"], context);
  assert.throws(() => rehearsalRunRequest(state, null, { mode: "design", sample: "Try", context: "\ud800" }), /valid UTF8/);
});

test("novice sample choices are bound to checked source and retain explicit fixtures without granting readiness", () => {
  const choice = { label: "Prevent duplicate titles", request: "Save Dune twice and show my wishlist.", fixtures: { clock: "sample time" } };
  const build = { guide_id: state.id, guide_version: state.version, current: true, status: "failed", build_id: "checked-build",
    source_verified: true, source_digest: "exact-source", trial_samples: { source_digest: "exact-source", cases: [choice] } };
  assert.deepEqual(applicationTrialSamples(build, state), [choice]);
  assert.notEqual(applicationTrialSamples(build, state)[0].fixtures, choice.fixtures);
  assert.deepEqual(applicationTrialSamples({ ...build, trial_samples: { source_digest: "exact-source",
    cases: [{ label: "Try local state", request: "Save a note" }] } }, state)[0].fixtures, {});
  for (const changes of [{ source_verified: false }, { source_digest: "changed-source" }, { current: false },
    { guide_version: 8 }, { status: "running" }, { trial_samples: { source_digest: "other", cases: [choice] } }])
    assert.deepEqual(applicationTrialSamples({ ...build, ...changes }, state), []);
  for (const fixtures of [null, [], { x: Infinity }, { x: NaN }]) assert.deepEqual(applicationTrialSamples({ ...build,
    trial_samples: { source_digest: "exact-source", cases: [{ ...choice, fixtures }] } }, state), []);
});

test("trial repair is offered only for a recorded problem on the exact terminal current application", () => {
  const build = { guide_id: state.id, guide_version: state.version, current: true, status: "failed", build_id: "checked-build",
    source_verified: true, source_digest: "exact-source" };
  const failed = { ...receipt, mode: "application", status: "completed", application_source: {
    build_id: "checked-build", source_digest: "exact-source" }, application_trial: { available: false } };
  const body = { version: state.version, rehearsal_id: receipt.rehearsal_id, build_id: "checked-build", source_digest: "exact-source" };
  assert.deepEqual(applicationTrialRepair(failed, build, state), body);
  assert.deepEqual(applicationTrialRepair({ ...failed, status: "unavailable" }, build, state), body);
  assert.deepEqual(applicationTrialRepair({ ...failed, application_trial: { available: true },
    application_observations: [{ kind: "callback_failed", error: "explicit failure" }] }, build, state), body);
  for (const changes of [{ mode: "design" }, { current: false }, { status: "running" }, { guide_version: 6 },
    { application_source: { build_id: "older-build", source_digest: "exact-source" } },
    { application_source: { build_id: "checked-build", source_digest: "old-source" } },
    { application_trial: { available: true } }]) assert.equal(applicationTrialRepair({ ...failed, ...changes }, build, state), null);
  assert.equal(applicationTrialRepair(failed, { ...build, source_verified: false }, state), null);
  assert.equal(applicationTrialRepair(failed, { ...build, build_id: "new-build" }, state), null);
  assert.equal(applicationTrialRepair(failed, { ...build, source_digest: "new-source" }, state), null);
});

test("completed staging-only trials allow an explicit improvement without inventing an automatic failure", () => {
  const build = { guide_id: state.id, guide_version: state.version, current: true, status: "completed", build_id: "checked-build",
    source_verified: true, source_digest: "exact-source" };
  const staged = { ...receipt, mode: "application", status: "completed", application_source: {
    build_id: "checked-build", source_digest: "exact-source" }, application_trial: { available: true },
    application_observations: [{ kind: "callback_returned", result: { status: "awaiting-human-confirmation" } }] };
  assert.equal(applicationTrialRepair(staged, build, state), null);
  const body = { version: state.version, rehearsal_id: receipt.rehearsal_id, build_id: "checked-build", source_digest: "exact-source" };
  assert.deepEqual(applicationTrialFeedback(staged, build, state), body);
  const feedback = "Let me try confirmation with an explicit fake human event.";
  assert.deepEqual(applicationTrialImprovement(staged, build, state, `  ${feedback}  `), { ...body, request: feedback });
  for (const invalid of ["", " ", null, 4, "x".repeat(6001)])
    assert.equal(applicationTrialImprovement(staged, build, state, invalid), null);
  for (const changes of [{ current: false }, { mode: "design" }, { status: "running" }, { guide_version: 6 },
    { application_source: { build_id: "old", source_digest: "exact-source" } }])
    assert.equal(applicationTrialImprovement({ ...staged, ...changes }, build, state, feedback), null);
  assert.equal(applicationTrialImprovement(staged, { ...build, source_digest: "new" }, state, feedback), null);
  assert.equal(applicationTrialImprovement(staged, { ...build, source_verified: false }, state, feedback), null);
});

test("partially missing offline callbacks offer the existing repair without treating live dependency prose as failure", () => {
  const build = { guide_id: state.id, guide_version: state.version, current: true, status: "completed", build_id: "checked-build",
    source_verified: true, source_digest: "exact-source" };
  const partial = { ...receipt, mode: "application", status: "completed", application_source: {
    build_id: "checked-build", source_digest: "exact-source" }, application_trial: { available: true, dependencies: [
      "owner:main: unavailable callbacks save_decision, delivery_status, save_decision",
      "A live connector needs credentials."] },
    application_observations: [{ kind: "callback_returned", result: { state: "empty" } }] };
  const body = { version: state.version, rehearsal_id: receipt.rehearsal_id, build_id: "checked-build", source_digest: "exact-source" };
  const original = structuredClone(partial);
  assert.deepEqual(applicationTrialRepair(partial, build, state), body);
  assert.equal(applicationTrialSummary(partial).unavailableCount, 2);
  assert.equal(applicationTrialSummary(partial).failedCount, 0);
  for (const changes of [{ current: false }, { status: "running" }, { mode: "design" },
    { application_source: { build_id: "old", source_digest: "exact-source" } }])
    assert.equal(applicationTrialRepair({ ...partial, ...changes }, build, state), null);
  assert.equal(applicationTrialRepair(partial, { ...build, source_digest: "new-source" }, state), null);
  assert.deepEqual(partial, original);
  const limitsOnly = { ...partial, application_trial: { available: true, dependencies: [
    "Live Slack needs credentials.", "Unsupported fake hardware I/O.",
    "owner:main: unavailable callbacks not a callback name", "owner\n:main: unavailable callbacks save_decision"] },
    application_observations: [{ kind: "callback_returned", result: {
      available: false, dependencies: ["owner:main: unavailable callbacks save_decision"] } }] };
  assert.equal(applicationTrialRepair(limitsOnly, build, state), null);
  assert.equal(applicationTrialSummary(limitsOnly).problem, false);
});

// The actual Astro trial finished its native trace but its only attempted
// application callback failed. Native completion must not obscure that result.
const callbackProblem = { ...receipt, mode: "application", status: "completed",
  detail: "The native trial completed. Replay the actual agent trace; live integrations remain unverified.",
  application_trial: { available: true, dependencies: [
    "lead:main: unavailable callbacks observing_deliver_telegram",
    "operator:main: unavailable callbacks observing_bounded_action"] },
  application_observations: [
    { kind: "callback_requested", identity: "lead:main", name: "observing_session_status" },
    { kind: "callback_failed", identity: "lead:main", name: "observing_session_status",
      detail: "BoundaryError: Unknown session" }] };

test("native completion and recorded callback problems have separate truthful primary summaries", () => {
  const before = structuredClone(callbackProblem);
  const summary = applicationTrialSummary(callbackProblem);
  assert.equal(summary.failedCount, 1);
  assert.equal(summary.unavailableCount, 2);
  assert.equal(summary.problem, true);
  assert.match(summary.text, /native trial finished/);
  assert.match(summary.text, /1 application callback failed/);
  assert.match(summary.text, /2 application callbacks were unavailable/);
  assert.deepEqual(callbackProblem, before);
  const pending = applicationTrialSummary({ ...callbackProblem, status: "running" });
  assert.match(pending.text, /native trial is running/);
  assert.doesNotMatch(pending.text, /finished|complete/);
  const unavailable = applicationTrialSummary({ mode: "application", status: "unavailable", application_trial: { available: false } });
  assert.match(unavailable.text, /could not start/);
  assert.match(unavailable.text, /callbacks were unavailable/);
  assert.equal(unavailable.unavailableCount, 0);
  assert.equal(applicationTrialSummary({ ...callbackProblem, mode: "design" }), null);
});

test("callback-returned values and arbitrary prose cannot become host failure or business completion claims", () => {
  const staged = { mode: "application", status: "completed", application_trial: { available: true,
    dependencies: ["Someone says all callbacks failed."] },
    application_observations: [{ kind: "callback_returned", result: { status: "awaiting-human-confirmation",
      failure: "callback_failed", ready_to_deploy: true } }] };
  const summary = applicationTrialSummary(staged);
  assert.equal(summary.failedCount, 0);
  assert.equal(summary.unavailableCount, 0);
  assert.equal(summary.problem, false);
  assert.match(summary.text, /offline observations, not proof that your job is complete/);
  assert.match(applicationTrialSummary({ ...staged, application_observations: [] }).text, /No application callback results/);
  const repeated = applicationTrialSummary({ ...staged, application_trial: { available: true, dependencies: [
    "lead:main: unavailable callbacks read_sample, read_sample", "other:main: unavailable callbacks read_sample"] },
    application_observations: Array.from({ length: 1005 }, () => ({ kind: "callback_failed" })) });
  assert.equal(repeated.unavailableCount, 2);
  assert.equal(repeated.failedCount, 1001);
  assert.match(repeated.text, /More than 1000 application callbacks failed/);
});

const summaryBundle = await build({ entryPoints: [fileURLToPath(new URL("rehearsal-panel.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", loader: { ".css": "empty" }, logLevel: "silent",
  nodePaths: [fileURLToPath(new URL("node_modules/", import.meta.url))],
  alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) } });
const { RehearsalTrialSummary, RehearsalContinuation, RehearsalSampleChoices, RehearsalApplicationPreview } = await import(
  `data:text/javascript;base64,${Buffer.from(summaryBundle.outputFiles[0].text).toString("base64")}`);
const renderSummary = (value, canRepair) => renderToStaticMarkup(React.createElement(RehearsalTrialSummary, { receipt: value, canRepair }));

test("sample cards show a bounded plain excerpt without changing a selected job or fixtures", () => {
  const authored = { label: "Compare two reading choices", request: "Compare these two choices.\n\n" +
    "Use the isolated example and explain the result. ".repeat(40) + "FINAL SAMPLE-ONLY CONFIRMATION",
    fixtures: { books: [{ title: "Exact sample title" }], confirmation: "synthetic-only" } };
  const before = structuredClone(authored);
  const build = { guide_id: state.id, guide_version: state.version, current: true, status: "completed",
    build_id: "checked", source_verified: true, source_digest: "source",
    trial_samples: { source_digest: "source", cases: [authored] } };
  const samples = applicationTrialSamples(build, state);
  let selected;
  const element = RehearsalSampleChoices({ samples, selected: null, disabled: false,
    onSelect: (item, index) => { selected = { item, index }; } });
  const html = renderToStaticMarkup(element);
  assert.match(html, /Compare two reading choices/);
  assert.match(html, /Compare these two choices\. Use the isolated example/);
  assert.ok(html.length < 650);
  assert.doesNotMatch(html, /FINAL SAMPLE-ONLY CONFIRMATION/);
  assert.doesNotMatch(html, /synthetic-only/);
  element.props.children[1][0].props.onClick();
  assert.equal(selected.index, 0);
  assert.equal(selected.item.request, authored.request);
  assert.deepEqual(selected.item.fixtures, authored.fixtures);
  const wire = rehearsalRunRequest(state, build, { mode: "application", sample: selected.item.request,
    applicationData: JSON.stringify(selected.item.fixtures) });
  assert.equal(wire.text, authored.request);
  assert.deepEqual(wire.application_fixtures, authored.fixtures);
  assert.deepEqual(authored, before);
});

test("sample preview bounds Unicode characters and escapes authored markup in the actual card", () => {
  assert.equal(rehearsalSamplePreview("  Short\n\tjob  "), "Short job");
  assert.equal(rehearsalSamplePreview(null), "");
  const preview = rehearsalSamplePreview("\u{1F52D}".repeat(181));
  assert.equal([...preview].length, 180);
  assert.equal(preview, "\u{1F52D}".repeat(177) + "...");
  const html = renderToStaticMarkup(React.createElement(RehearsalSampleChoices,
    { samples: [{ label: "A sample", request: "<script>example</script>", fixtures: {} }],
      selected: 0, disabled: true, onSelect: () => assert.fail("Rendering must not select a sample") }));
  assert.match(html, /&lt;script&gt;example&lt;\/script&gt;/);
  assert.doesNotMatch(html, /<script>/);
  assert.match(html, /aria-pressed="true" disabled=""/);
});

test("the production summary exposes recorded failure before native history and keeps repair eligibility external", () => {
  const html = renderSummary(callbackProblem, true);
  const primary = html.slice(0, html.indexOf("<details>"));
  assert.match(primary, /class="implementation-warning" role="status"/);
  assert.match(primary, /1 application callback failed/);
  assert.match(primary, /2 application callbacks were unavailable/);
  assert.match(primary, /Choose Fix this trial/);
  assert.ok(!primary.includes(callbackProblem.detail));
  assert.ok(html.includes(callbackProblem.detail));
  assert.match(html, /<details><summary>Saved native trial result/);
  assert.doesNotMatch(html, /<details[^>]*\bopen\b/);
  assert.doesNotMatch(renderSummary(callbackProblem, false), /Choose Fix this trial/);
  const escaped = renderSummary({ ...callbackProblem, detail: "<script>historical text</script>" }, false);
  assert.match(escaped, /&lt;script&gt;historical text&lt;\/script&gt;/);
  assert.doesNotMatch(escaped, /<script>/);
  assert.match(renderSummary({ ...receipt, mode: "design", detail: "Exact saved design result" }, false), /Exact saved design result/);
});

const trialBuild = { guide_id: state.id, guide_version: state.version, current: true, status: "completed",
  build_id: "checked-build", source_verified: true, source_digest: "exact-source" };
const previewBuild = { ...trialBuild, source_digest: "a".repeat(64), status: "failed",
  acceptance: { status: "needs_repair" } };
const previewDocument = '<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="default-src \'none\'; script-src \'none\'"></head><body><main><h1>Sample collection</h1><p>No sample books yet.</p></main></body></html>';
const previewTrial = { ...receipt, mode: "application", status: "waiting_input",
  application_source: { build_id: previewBuild.build_id, source_digest: previewBuild.source_digest },
  application_observation_revision: 3, application_preview: {
    status: "available", scope: "offline-application-preview", sanitization: "inert-html-v1", live_ready: false,
    trial_id: receipt.rehearsal_id, build_id: previewBuild.build_id, source_digest: previewBuild.source_digest,
    guide_version: receipt.guide_version, observation_revision: 3, title: "Sample collection",
    html: previewDocument, document_sha256: "b".repeat(64), detail: null } };
const renderPreview = (value = previewTrial, selectedBuild = previewBuild, selectedState = state, replaying = false) =>
  renderToStaticMarkup(React.createElement(RehearsalApplicationPreview,
    { preview: applicationTrialPreview(value, selectedBuild, selectedState), replaying }));

test("the production preview frame is inert, source-bound and separate from acceptance or live UI evidence", () => {
  const before = structuredClone(previewTrial), model = applicationTrialPreview(previewTrial, previewBuild, state);
  assert.equal(model.html, previewDocument);
  const view = RehearsalApplicationPreview({ preview: model, replaying: false });
  const frame = view.props.children[2].props.children[1];
  assert.equal(frame.type, "iframe");
  assert.equal(frame.props.sandbox, "");
  assert.equal(frame.props.referrerPolicy, "no-referrer");
  assert.equal(frame.props.srcDoc, previewDocument);
  for (const field of ["src", "allow", "onLoad", "onMessage"]) assert.equal(Object.hasOwn(frame.props, field), false);
  const html = renderPreview();
  assert.match(html, /Offline application preview/);
  assert.match(html, /Sample state\. You can expand details; application controls are inactive\./);
  assert.match(html, /does not verify live UI interaction, authentication or deployment/);
  assert.match(html, /sandbox=""/);
  assert.doesNotMatch(html, /allow-scripts|allow-same-origin|needs_repair|ready_to_deploy|<form/);
  assert.deepEqual(previewTrial, before);
});

test("old trial, source, guide and malformed revision evidence clears the production preview instead of borrowing it", () => {
  for (const change of [{ mode: "design" }, { current: false }, { guide_id: "other" }, { guide_version: 6 },
    { rehearsal_id: "new-trial" }, { application_preview: undefined },
    { application_source: { build_id: "another", source_digest: previewBuild.source_digest } },
    { application_observation_revision: undefined }, { application_observation_revision: true },
    { application_observation_revision: "3" }, { application_observation_revision: -1 }]) {
    assert.equal(renderPreview({ ...previewTrial, ...change }), "");
  }
  for (const change of [{ trial_id: "old-trial" }, { build_id: "old-build" }, { source_digest: "c".repeat(64) },
    { guide_version: 6 }, { guide_version: "7" }, { observation_revision: 2 }, { observation_revision: 4 },
    { observation_revision: true }, { scope: "declared-source" }, { sanitization: undefined },
    { sanitization: "interactive-v1" }, { live_ready: true }, { live_ready: undefined }, { status: "running" }]) {
    assert.equal(renderPreview({ ...previewTrial, application_preview: { ...previewTrial.application_preview, ...change } }), "");
  }
  for (const change of [{ build_id: "new-build" }, { source_digest: "c".repeat(64) }, { source_verified: false },
    { current: false }, { status: "checking" }]) assert.equal(renderPreview(previewTrial, { ...previewBuild, ...change }), "");
  assert.equal(renderPreview(previewTrial, previewBuild, { ...state, id: "other" }), "");
});

test("an advancing callback observation removes old HTML and shows only its matching new sample view", () => {
  const oldModel = applicationTrialPreview(previewTrial, previewBuild, state);
  const advanced = { ...previewTrial, application_observation_revision: 4 };
  assert.equal(renderPreview(advanced), "");
  const latest = { ...advanced, application_preview: { ...advanced.application_preview, observation_revision: 4,
    html: previewDocument.replace("No sample books yet.", "One sample book saved."), document_sha256: "c".repeat(64) } };
  const newModel = applicationTrialPreview(latest, previewBuild, state);
  assert.notEqual(newModel.key, oldModel.key);
  assert.match(renderPreview(latest), /One sample book saved/);
  assert.doesNotMatch(renderPreview(latest), /No sample books yet/);
  assert.match(renderPreview(latest, previewBuild, state, true), /agent replay may show an earlier event/);
  assert.equal(applicationTrialPreview({ ...latest, status: "completed", worker_stop_confirmed: true,
    application_stop_confirmed: true }, previewBuild, state).html, newModel.html);
});

test("preview provenance keeps the original frozen guide version across a harmless current chat revision", () => {
  const guide = { ...state, version: 8 }, current = { ...previewTrial, current_guide_version: 8 };
  const selectedBuild = { ...previewBuild, current_guide_version: 8 };
  assert.ok(applicationTrialPreview(current, selectedBuild, guide));
  assert.equal(renderPreview({ ...current, application_preview: { ...current.application_preview, guide_version: 8 } },
    selectedBuild, guide), "");
});

test("unsupported and failed previews are relevant current-trial notes without frames, raw exceptions or ready claims", () => {
  for (const status of ["unsupported", "failed"]) {
    const value = { ...previewTrial, application_preview: { ...previewTrial.application_preview,
      status, title: null, html: null, document_sha256: null, detail: null,
      raw_error: "SECRET-RUNTIME-EXCEPTION", configured: true, ready: true } };
    const html = renderPreview(value);
    assert.match(html, /role="status"/);
    assert.match(html, /trial&#x27;s outcome is shown separately/);
    assert.doesNotMatch(html, /<iframe|SECRET-RUNTIME-EXCEPTION|configured|ready="/);
    assert.equal(renderPreview({ ...value, current: false }), "");
    assert.equal(renderPreview({ ...value, application_preview: { ...value.application_preview, html: previewDocument } }), "");
    assert.equal(renderPreview({ ...value, application_preview: { ...value.application_preview, document_sha256: "b".repeat(64) } }), "");
  }
  const detail = '<span>This source has no offline view.</span>';
  const html = renderPreview({ ...previewTrial, application_preview: { ...previewTrial.application_preview,
    status: "unsupported", title: null, html: null, document_sha256: null, detail } });
  assert.match(html, /&lt;span&gt;This source has no offline view\.&lt;\/span&gt;/);
  assert.doesNotMatch(html, /<span>This source/);
});

test("preview document and title bounds use UTF-8 bytes and Unicode characters without silently truncating the view", () => {
  for (const change of [{ html: null }, { html: "" }, { html: 12 }, { html: "<p>\u0000</p>" },
    { html: "\u{1F52D}".repeat(25_001) }, { document_sha256: null }, { document_sha256: "not-a-hash" },
    { title: null }, { title: "" }, { title: "\u{1F52D}".repeat(121) }, { title: "Line\nbreak" }, { title: { unsafe: true } }]) {
    assert.equal(renderPreview({ ...previewTrial, application_preview: { ...previewTrial.application_preview, ...change } }), "");
  }
  assert.ok(applicationTrialPreview({ ...previewTrial, application_preview: { ...previewTrial.application_preview,
    title: "\u{1F52D}".repeat(120), html: "\u{1F52D}".repeat(25_000) } }, previewBuild, state));
  const html = renderPreview({ ...previewTrial, application_preview: { ...previewTrial.application_preview,
    title: '<script>Sample title</script>' } });
  assert.match(html, /&lt;script&gt;Sample title&lt;\/script&gt;/);
  assert.doesNotMatch(html, /<script>Sample title/);
});

const waitingTrial = { ...receipt, mode: "application", keep_open: true, status: "waiting_input", can_continue: true,
  turn_count: 1, current_turn: 1, worker_stop_confirmed: false,
  application_source: { build_id: trialBuild.build_id, source_digest: trialBuild.source_digest },
  idle_expires_at: "2026-10-01T12:05:00Z", expires_at: "2026-10-01T12:10:00Z" };
const admittedFollowUp = { ...waitingTrial, status: "running", can_continue: false, current_turn: 2,
  turn_command: { id: "owned-command", turn: 2, status: "sent" } };

test("only a current waiting callback problem offers End-to-repair guidance without authorizing repair", () => {
  const failed = { ...waitingTrial, application_observations: [{ kind: "callback_failed" }] };
  assert.equal(applicationTrialEndToRepair(failed, trialBuild, state), true);
  assert.equal(applicationTrialRepair(failed, trialBuild, state), null);
  for (const changed of [{ current: false }, { status: "running" }, { keep_open: false },
    { application_source: { build_id: "other", source_digest: trialBuild.source_digest } }])
    assert.equal(applicationTrialEndToRepair({ ...failed, ...changed }, trialBuild, state), false);
  assert.equal(applicationTrialEndToRepair(waitingTrial, trialBuild, state), false);
  const html = renderToStaticMarkup(React.createElement(RehearsalContinuation, { receipt: failed,
    eligible: true, text: "", endToRepair: applicationTrialEndToRepair(failed, trialBuild, state) }));
  assert.match(html, /Once the host confirms cleanup/);
  assert.match(html, />End trial<\/button>/);
  assert.doesNotMatch(html, />Fix this trial<\/button>/);
});

test("only the current exact application and host-owned waiting lane can continue", () => {
  assert.equal(rehearsalActive(waitingTrial), true);
  assert.equal(rehearsalCanContinue(waitingTrial, trialBuild, state), true);
  assert.deepEqual(rehearsalContinueRequest(waitingTrial, trialBuild, state, "  What happened to that choice?  "),
    { version: state.version, rehearsal_id: receipt.rehearsal_id, expected_turn: 1, text: "What happened to that choice?" });
  for (const change of [{ keep_open: false }, { status: "completed" }, { status: "running" }, { status: "ending" },
    { can_continue: false }, { can_continue: undefined }, { current: false }, { guide_id: "other" },
    { guide_version: 6 }, { turn_count: 0 }, { turn_count: "1" }, { turn_count: true }, { rehearsal_id: "" },
    { worker_stop_confirmed: true }, { flow_id: "actual-flow" }, { mode: "unknown" },
    { application_source: { build_id: "another-build", source_digest: trialBuild.source_digest } },
    { application_source: { build_id: trialBuild.build_id, source_digest: "changed-source" } }]) {
    assert.equal(rehearsalCanContinue({ ...waitingTrial, ...change }, trialBuild, state), false);
    assert.equal(rehearsalContinueRequest({ ...waitingTrial, ...change }, trialBuild, state, "Follow up"), null);
  }
  for (const change of [{ source_verified: false }, { current: false }, { status: "running" },
    { build_id: "another-build" }, { source_digest: "changed-source" }])
    assert.equal(rehearsalCanContinue(waitingTrial, { ...trialBuild, ...change }, state), false);
  for (const text of [null, 4, "", "  ", "x".repeat(6001)])
    assert.equal(rehearsalContinueRequest(waitingTrial, trialBuild, state, text), null);
  const design = { ...waitingTrial, mode: "design", application_source: undefined };
  assert.equal(rehearsalCanContinue(design, null, state), true);
  assert.equal(rehearsalCanContinue({ ...design, current: false }, null, state), false);
});

function followUpHarness() {
  const epoch = createRehearsalReceiptEpoch(state.id), response = delayedResponse();
  let context = { receipt: waitingTrial, build: trialBuild, state, text: "Confirm the previous choice." };
  const writes = [], errors = [], bodies = [];
  return { epoch, response, writes, errors, bodies, get context() { return context; },
    set context(value) { context = value; },
    admit() { return admitRehearsalFollowUp({ ...context, epoch, load: (body) => { bodies.push(body); return response.promise; },
      current: () => context, onReceipt: (value) => { context = { ...context, receipt: value }; writes.push(value); },
      onClear: () => { context = { ...context, text: "" }; }, onError: (error) => errors.push(error.message) }); } };
}

test("an acknowledged same-trial command clears only its unchanged draft and leaves real history in place", async () => {
  const harness = followUpHarness(), original = structuredClone(waitingTrial);
  const pending = harness.admit();
  assert.equal(harness.context.text, "Confirm the previous choice.");
  assert.deepEqual(harness.bodies, [{ version: 7, rehearsal_id: "trial", expected_turn: 1, text: "Confirm the previous choice." }]);
  harness.response.resolve(admittedFollowUp);
  assert.equal(await pending, true);
  assert.equal(harness.context.text, "");
  assert.equal(harness.writes[0], admittedFollowUp);
  assert.deepEqual(waitingTrial, original);
  assert.deepEqual(harness.errors, []);

  const edited = followUpHarness(), admission = edited.admit();
  edited.context = { ...edited.context, text: "A new draft written while sending." };
  edited.response.resolve(admittedFollowUp);
  assert.equal(await admission, true);
  assert.equal(edited.context.text, "A new draft written while sending.");
  const fast = followUpHarness(), faster = fast.admit();
  fast.response.resolve({ ...admittedFollowUp, turn_command: { ...admittedFollowUp.turn_command, status: "acknowledged" } });
  assert.equal(await faster, true);
  assert.equal(fast.context.text, "");
});

test("passive polls cannot replace an admission while it is in flight and resume afterward", async () => {
  const harness = followUpHarness(), pendingRead = delayedResponse();
  const reads = [], unavailable = [];
  const callbacks = { onReceipt: (value) => reads.push(value), onUnavailable: (value) => unavailable.push(value) };
  const old = harness.epoch.read(() => pendingRead.promise, () => state, callbacks);
  const admission = harness.admit();
  let calls = 0;
  assert.equal(await harness.epoch.read(async () => { calls += 1; return waitingTrial; }, () => state, callbacks), false);
  assert.equal(calls, 0);
  pendingRead.resolve(waitingTrial);
  assert.equal(await old, false);
  harness.response.resolve(admittedFollowUp);
  assert.equal(await admission, true);
  assert.deepEqual(reads, []);
  assert.deepEqual(unavailable, []);
  const next = { ...waitingTrial, turn_count: 2, current_turn: 2 };
  assert.equal(await harness.epoch.read(async () => next, () => state, callbacks), true);
  assert.equal(reads[0], next);
  assert.deepEqual(unavailable, [false]);
  const failed = followUpHarness(), failure = failed.admit();
  failed.response.reject(new Error("Rejected"));
  assert.equal(await failure, false);
  assert.equal(await failed.epoch.read(async () => waitingTrial, () => state, callbacks), true);
});

test("failed or unacknowledged follow-ups preserve the draft and cannot invent a native turn", async () => {
  const failed = followUpHarness(), pending = failed.admit();
  failed.response.reject(new Error("Actual admission failed"));
  assert.equal(await pending, false);
  assert.equal(failed.context.text, "Confirm the previous choice.");
  assert.deepEqual(failed.writes, []);
  assert.deepEqual(failed.errors, ["Actual admission failed"]);
  for (const value of [{ ...admittedFollowUp, rehearsal_id: "other-trial" },
    { ...admittedFollowUp, turn_command: undefined },
    { ...admittedFollowUp, turn_command: { id: "command", turn: 1, status: "sent" } },
    { ...admittedFollowUp, turn_command: { id: "command", turn: 2, status: "completed" } },
    { ...admittedFollowUp, application_source: { ...waitingTrial.application_source, source_digest: "other" } }]) {
    const harness = followUpHarness(), response = harness.admit();
    harness.response.resolve(value);
    assert.equal(await response, false);
    assert.equal(harness.context.text, "Confirm the previous choice.");
    assert.equal(harness.writes.length, 0);
    assert.match(harness.errors[0], /not acknowledged for this trial/);
  }
});

test("delayed continuation results cannot replace another trial, guide, source, epoch or unmounted view", async () => {
  for (const change of ["trial", "guide", "source", "epoch", "unmount"]) {
    const harness = followUpHarness(), pending = harness.admit();
    if (change === "trial") harness.context = { ...harness.context, receipt: { ...waitingTrial, rehearsal_id: "new-trial" } };
    if (change === "guide") harness.context = { ...harness.context, state: { ...state, version: 8 } };
    if (change === "source") harness.context = { ...harness.context, build: { ...trialBuild, source_digest: "new-source" } };
    if (change === "epoch") harness.epoch.invalidate();
    if (change === "unmount") harness.epoch.stop();
    harness.response.resolve(admittedFollowUp);
    assert.equal(await pending, false);
    assert.equal(harness.context.text, "Confirm the previous choice.");
    assert.equal(harness.writes.length, 0);
    assert.deepEqual(harness.errors, []);
  }
  const failed = followUpHarness(), pending = failed.admit();
  failed.context = { ...failed.context, receipt: { ...waitingTrial, rehearsal_id: "new-trial" } };
  failed.response.reject(new Error("Old trial failed"));
  assert.equal(await pending, false);
  assert.deepEqual(failed.errors, []);
  const unmounted = followUpHarness();
  unmounted.epoch.stop();
  assert.equal(await unmounted.admit(), false);
  assert.deepEqual(unmounted.bodies, []);
});

test("trials with terminal turns cannot offer builder feedback before actual worker cleanup", () => {
  const problem = { ...waitingTrial, status: "completed", application_observations: [{ kind: "callback_failed" }] };
  for (const flag of [undefined, null, false]) {
    assert.equal(applicationTrialFeedback({ ...problem, worker_stop_confirmed: flag }, trialBuild, state), null);
    assert.equal(applicationTrialRepair({ ...problem, worker_stop_confirmed: flag }, trialBuild, state), null);
  }
  const stopped = { ...problem, worker_stop_confirmed: true, application_stop_confirmed: true };
  assert.ok(applicationTrialRepair(stopped, trialBuild, state));
  assert.ok(applicationTrialImprovement(stopped, trialBuild, state, "Improve this behavior."));
  for (const flag of [undefined, null, false]) {
    const unconfirmed = { ...stopped, application_stop_confirmed: flag };
    assert.equal(applicationTrialFeedback(unconfirmed, trialBuild, state), null);
    assert.equal(applicationTrialRepair(unconfirmed, trialBuild, state), null);
    assert.equal(applicationTrialImprovement(unconfirmed, trialBuild, state, "Improve this behavior."), null);
  }
  const missing = { ...stopped };
  delete missing.application_stop_confirmed;
  assert.equal(applicationTrialFeedback(missing, trialBuild, state), null);
  assert.equal(applicationTrialRepair(missing, trialBuild, state), null);
  assert.equal(applicationTrialImprovement(missing, trialBuild, state, "Improve this behavior."), null);
  const legacy = { ...problem, keep_open: undefined };
  delete legacy.worker_stop_confirmed;
  assert.ok(applicationTrialRepair(legacy, trialBuild, state));
  assert.ok(applicationTrialImprovement(legacy, trialBuild, state, "Improve the one-shot behavior."));
  for (const flag of [undefined, null, false])
    assert.equal(applicationTrialRepair({ ...problem, keep_open: false, worker_stop_confirmed: flag }, trialBuild, state), null);
  assert.equal(applicationTrialFeedback(waitingTrial, trialBuild, state), null);
});

const continuationProps = { receipt: waitingTrial, eligible: true, text: "Ask about the previous answer.",
  onText: () => {}, onContinue: () => {}, onEnd: () => {}, continuing: false, ending: false, blocked: false };
const renderContinuation = (changes = {}) => renderToStaticMarkup(React.createElement(RehearsalContinuation,
  { ...continuationProps, ...changes }));

test("the production waiting composer exposes one explicit continuation and end without synthesizing conversation", () => {
  const html = renderContinuation();
  assert.match(html, /same isolated conversation and sample state/);
  assert.match(html, /What would you ask next/);
  assert.match(html, /Continue this trial/);
  assert.match(html, /End trial/);
  assert.match(html, /maxLength="6000"/);
  assert.match(html, /Follow-up window ends <time dateTime="2026-10-01T12:05:00Z"/i);
  assert.doesNotMatch(html, /Fix this trial|Improve from this trial|approved|application is ready/);
  assert.match(renderContinuation({ eligible: false }), /cannot accept a follow-up/);
  assert.match(renderContinuation({ eligible: false }), /<button class="setup-action" disabled=""/);
  assert.match(renderContinuation({ continuing: true }), /Sending follow-up/);
  assert.match(renderContinuation({ receipt: admittedFollowUp }), /agents are working/);
  assert.doesNotMatch(renderContinuation({ receipt: admittedFollowUp }), /What would you ask next|Continue this trial/);
  assert.match(renderContinuation({ receipt: { ...waitingTrial, status: "ending" } }), /Ending this trial and stopping/);
});

test("the production composer forwards only explicit follow-up, end and draft actions", () => {
  const calls = [];
  const element = RehearsalContinuation({ ...continuationProps,
    onText: (value) => calls.push(["draft", value]), onContinue: () => calls.push(["continue"]), onEnd: () => calls.push(["end"]) });
  const descendants = (node) => node && typeof node === "object"
    ? [node, ...React.Children.toArray(node.props?.children).flatMap(descendants)] : [];
  const nodes = descendants(element);
  assert.deepEqual(calls, []);
  nodes.find((node) => node.type === "textarea").props.onChange({ target: { value: "A specific follow-up." } });
  nodes.find((node) => node.type === "button" && node.props.className === "setup-action").props.onClick();
  nodes.find((node) => node.type === "button" && node.props.className === "implementation-link").props.onClick();
  assert.deepEqual(calls, [["draft", "A specific follow-up."], ["continue"], ["end"]]);
});

test("lifetime and ended projections keep one-shot history intact and never claim unconfirmed cleanup", () => {
  assert.equal(rehearsalSessionSummary({ ...receipt, status: "completed" }), null);
  assert.equal(renderContinuation({ receipt: { ...receipt, status: "completed" } }), "");
  const unknown = { ...waitingTrial, status: "completed", ended_reason: "<script>invented completion</script>", worker_stop_confirmed: false };
  assert.match(renderContinuation({ receipt: unknown }), /cleanup has not been confirmed/);
  assert.doesNotMatch(renderContinuation({ receipt: unknown }), /Continue this trial|End trial|<script>/);
  const expired = { ...waitingTrial, status: "completed", ended_reason: "idle_timeout", worker_stop_confirmed: true,
    application_stop_confirmed: true };
  assert.match(renderContinuation({ receipt: expired }), /ended after waiting for a follow-up/);
  assert.match(renderContinuation({ receipt: expired }), /workers have stopped/);
  assert.doesNotMatch(renderContinuation({ receipt: expired }), /Follow-up window ends|Trial time limit/);
  for (const flag of [false, undefined]) {
    const uncertain = renderContinuation({ receipt: { ...expired, application_stop_confirmed: flag } });
    assert.match(uncertain, /native worker has stopped/);
    assert.match(uncertain, /Application cleanup has not been confirmed/);
    assert.doesNotMatch(uncertain, /Its workers have stopped/);
  }
  assert.equal(rehearsalSessionSummary({ ...waitingTrial, expires_at: "not a timestamp", idle_expires_at: {} }).expiresAt, null);
});

test("the production summary offers investigation when only individually missing offline callbacks were recorded", () => {
  const build = { guide_id: state.id, guide_version: state.version, current: true, status: "completed", build_id: "checked-build",
    source_verified: true, source_digest: "exact-source" };
  const partial = { ...receipt, mode: "application", status: "completed", application_source: {
    build_id: "checked-build", source_digest: "exact-source" }, application_trial: { available: true,
      dependencies: ["owner:main: unavailable callbacks save_decision"] }, application_observations: [] };
  const html = renderSummary(partial, !!applicationTrialRepair(partial, build, state));
  assert.match(html, /1 application callback was unavailable/);
  assert.match(html, /Choose Fix this trial to ask the builder to investigate/);
  assert.doesNotMatch(html, /application callback failed|job is complete/);
});
