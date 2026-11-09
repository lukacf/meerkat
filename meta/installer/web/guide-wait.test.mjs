import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { build } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import { guideWaitBinding, guideWaitElapsed } from "./guide-wait.mjs";

const require = createRequire(import.meta.url);
const bundled = await build({ entryPoints: [fileURLToPath(new URL("guide-wait.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", jsx: "automatic", logLevel: "silent",
  plugins: [{ name: "shared-real-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true,
    }));
  } }],
});
const { GuideWaitStatus } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);

test("wait timing uses the saved admission without inventing invalid or future durations", () => {
  const now = Date.parse("2026-10-02T12:00:00Z");
  for (const value of [null, undefined, "", "not a date", "2026-10-02T12:00:01Z", 0]) {
    assert.equal(guideWaitElapsed(value, now), null);
  }
  assert.equal(guideWaitElapsed("2026-10-02T11:58:45Z", now), "1m 15s");
  assert.equal(guideWaitElapsed("2026-10-02T11:57:43Z", now), "2m 17s");
  assert.equal(guideWaitElapsed("2026-10-02T12:00:00Z", now), "0s");
  assert.equal(guideWaitElapsed("2026-10-02T10:57:59Z", now), "1h 2m");
  assert.equal(guideWaitElapsed("2026-10-02T12:00:00Z", NaN), null);
});

test("only a running saved request supplies a wait identity and every binding field resets it", () => {
  const state = { id: "owned-guide", operation: { status: "running", request_id: "owned-request",
    started_at: "2026-10-02T12:00:00Z" } };
  const before = structuredClone(state), initial = guideWaitBinding(state);
  assert.deepEqual(initial, { guideId: state.id, requestId: state.operation.request_id,
    startedAt: state.operation.started_at,
    key: JSON.stringify([state.id, state.operation.request_id, state.operation.started_at]) });
  for (const status of ["completed", "failed", "interrupted", undefined]) {
    assert.equal(guideWaitBinding({ ...state, operation: { ...state.operation, status } }), null);
  }
  assert.equal(guideWaitBinding({ ...state, id: null }), null);
  assert.equal(guideWaitBinding({ ...state, operation: { ...state.operation, request_id: "" } }), null);
  for (const changed of [{ ...state, id: "other-guide" },
    { ...state, operation: { ...state.operation, request_id: "other-request" } },
    { ...state, operation: { ...state.operation, started_at: "2026-10-02T12:00:05Z" } }]) {
    assert.notEqual(guideWaitBinding(changed).key, initial.key);
  }
  assert.deepEqual(state, before);
});

test("following is offered only for the current running request with available live observation", () => {
  const state = { id: "owned-guide", operation: { status: "running", request_id: "owned-request" } };
  const snapshot = { guideId: state.id, available: true };
  let calls = 0;
  const props = { state, snapshot, following: false, onFollow: () => { calls += 1; } };
  const render = (changes = {}) => renderToStaticMarkup(React.createElement(GuideWaitStatus, { ...props, ...changes }));
  assert.match(render(), /Follow live activity/);
  for (const changes of [{ following: true }, { onFollow: undefined }, { snapshot: { ...snapshot, available: false } },
    { snapshot: { ...snapshot, guideId: "other-guide" } }, { snapshot: { ...snapshot, error: "Interrupted observation" } },
    { snapshot: { ...snapshot, startup: { status: "starting" } } },
    { state: { ...state, operation: { ...state.operation, status: "completed" } } }]) {
    assert.doesNotMatch(render(changes), /Follow live activity/);
  }
  assert.equal(calls, 0, "Showing the return action admits no work");
});

test("wait status retains a confirmed tool label without claiming proposal completion or exposing tool content", () => {
  const state = {id: "owned-guide", operation: {status: "running", request_id: "owned-request",
    started_at: "2026-10-02T12:00:00Z"}};
  const snapshot = {guideId: state.id, available: true, bindings: [{interaction_id: "native-turn",
    operation_request_id: state.operation.request_id, operation_started_at: state.operation.started_at}],
  frames: [{id: "tool", event: "tool_call_requested", interactionId: "native-turn", timestampMs: 1000,
    data: {id: "call", name: "read_design_reference", args: {secret: "PRIVATE_ARGUMENT"}}},
  {id: "result", event: "tool_result_received", interactionId: "native-turn", timestampMs: 1001,
    data: {id: "call", name: "read_design_reference", is_error: false, result: "PRIVATE_RESULT"}}]};
  const render = (changed = snapshot) => renderToStaticMarkup(React.createElement(GuideWaitStatus, {state, snapshot: changed}));
  assert.match(render(), /Last tool: read design reference - completed/);
  assert.match(render(), /Guide request running/);
  assert.doesNotMatch(render(), /PRIVATE_ARGUMENT|PRIVATE_RESULT|Proposal completed/);
  assert.doesNotMatch(render({...snapshot, guideId: "different-guide"}), /Last tool/);
  assert.doesNotMatch(render({...snapshot, available: false}), /Last tool/);
});

test("a saved first idea uses one status while later turns retain their presentation and the timer stays quiet", () => {
  const state = { id: "owned-guide", operation: { status: "running", request_id: "owned-request",
    started_at: new Date(Date.now() - 20_000).toISOString() } };
  const snapshot = { guideId: state.id, available: true };
  const props = { state, snapshot, following: false, onFollow: () => assert.fail("Rendering must not follow") };
  const render = (changes = {}) => renderToStaticMarkup(React.createElement(GuideWaitStatus, { ...props, ...changes }));
  const laterCopy = "Your submitted answer is saved. You can keep writing below.";
  const first = render({ firstIdea: true });
  assert.match(first, /<p role="status">Saved - your guide is working<\/p>/);
  assert.doesNotMatch(first, /Guide request running|Your idea is saved|write your next message|<small>/);
  assert.ok(!first.includes(laterCopy));
  assert.match(first, /<span class="setup-guide-wait-clock" aria-live="off">\d+s elapsed<\/span>/);
  assert.match(first, /Follow live activity/, "The compact status retains the existing return action");
  assert.match(render(), /Guide request running/);
  assert.ok(render().includes(laterCopy));
  assert.equal(render({ firstIdea: false }), render(), "The default preserves later-turn copy");
});

test("retained tool activity is qualified while live updates or startup recover", () => {
  const state = { id: "owned-guide", operation: { status: "running", request_id: "owned-request",
    started_at: "2026-10-02T12:00:00Z" } };
  const snapshot = { guideId: state.id, available: true, bindings: [{ interaction_id: "native-turn",
    operation_request_id: state.operation.request_id, operation_started_at: state.operation.started_at }],
    frames: [{ id: "tool", event: "tool_call_requested", interactionId: "native-turn", timestampMs: 1000,
      data: { id: "call", name: "read_design_reference", args: { secret: "PRIVATE_ARGUMENT" } } }] };
  const before = structuredClone({ state, snapshot });
  const render = (changes = {}) => renderToStaticMarkup(React.createElement(GuideWaitStatus, {
    state, snapshot: { ...snapshot, ...changes }, firstIdea: true,
  }));
  assert.match(render(), /Using tool: read design reference/);
  for (const recovery of [{ error: "Interrupted observation" }, { retrying: true },
    { startup: { retryRequired: true } }, { startup: { status: "starting" } },
    { startup: { available: false } }]) {
    const markup = render(recovery);
    assert.match(markup, /Last observed tool: read design reference/);
    assert.match(markup, /Your submitted answer is saved/);
    assert.doesNotMatch(markup, /Using tool|PRIVATE_ARGUMENT|Saved - your guide is working/);
  }
  assert.match(render({ error: "" }), /Using tool: read design reference/,
    "A recovered timeline restores the current activity caption");
  assert.doesNotMatch(render({ guideId: "another-guide", error: "Interrupted observation" }), /observed tool|Using tool/);
  assert.deepEqual({ state, snapshot }, before, "Presentation preserves the native frames and request binding");
});

test("first idea copy does not claim an unconfirmed submission or replace startup and reconnecting guidance", () => {
  const state = { id: "owned-guide", operation: { status: "running", request_id: "owned-request" } };
  const snapshot = { guideId: state.id, available: true };
  const render = (changes = {}) => renderToStaticMarkup(React.createElement(GuideWaitStatus, {
    state, snapshot, firstIdea: true, ...changes,
  }));
  for (const unconfirmed of [{ id: state.id }, { ...state, operation: { status: "running", request_id: "" } }]) {
    const sending = render({ state: unconfirmed, submitting: true });
    assert.match(sending, /Sending to your guide/);
    assert.match(sending, /Waiting for confirmation\. You can keep writing below\./);
    assert.doesNotMatch(sending, /is saved|Saved -|will reply here|next draft/);
  }
  for (const recovery of [{ retrying: true }, { startup: { retryRequired: true } },
    { startup: { status: "starting" } }, { startup: { available: false } }, { error: "Interrupted observation" }]) {
    const current = { ...snapshot, ...recovery };
    assert.equal(render({ snapshot: current }), render({ snapshot: current, firstIdea: false }),
      "Startup and reconnecting guidance remain unchanged");
    assert.doesNotMatch(render({ snapshot: current }), /Saved -|will reply here|next draft/);
  }
});
