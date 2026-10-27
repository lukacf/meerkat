import test from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";

// Exercise the shipping Console parser and adapter through the same bundler
// used by this application, rather than substituting a fake chat formatter.
const built = await build({
  entryPoints: [fileURLToPath(new URL("native-guide-chat.mjs", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent",
});
const { createGuideFrameLog, ingestGuideTimeline, guideConversation, nextGuideTimelineCursor, guideCurrentToolActivity } =
  await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);

const require = createRequire(import.meta.url);
const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
const mountedBundle = await build({
  entryPoints: [fileURLToPath(new URL("native-guide-chat.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", jsx: "automatic", logLevel: "silent",
  loader: { ".css": "empty" }, nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
  alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
  plugins: [{ name: "shared-mounted-react", setup(builder) {
    builder.onResolve({ filter: /^(?:react|react-dom)(?:\/.*)?$/ }, args => ({
      path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }],
});
const { NativeGuideChat, useGuideTimeline } =
  await import(`data:text/javascript;base64,${Buffer.from(mountedBundle.outputFiles[0].text).toString("base64")}`);

const frame = (id, kind, payload, offset = 0) => ({
  id, kind, payload, interaction_id: "turn-one", identity: "architect:main",
  timestamp_ms: 1000 + offset, cursor: `console:${offset + 1}`, frame_version: 1,
  source: {kind: "runtime"},
});
const binding = {interaction_id: "turn-one", user_message_id: "saved-user", user_text: "Help my team",
  operation_request_id: "a".repeat(64), operation_started_at: "2026-10-04T09:00:00Z"};
const state = {
  id: "setup-one", operation: {status: "running", request_id: binding.operation_request_id,
    started_at: binding.operation_started_at}, messages: [],
};
const allBlocks = (entries) => entries.flatMap((entry) => entry.blocks || []);

test("wait activity projects only the exactly bound request's public tool name and checked status", () => {
  const log = createGuideFrameLog();
  let frames = ingestGuideTimeline(log, {frames: [
    frame("current-tool", "tool_call_requested", {id: "call", name: "inspect_blueprint", args: {private: "not a progress label"}}, 1),
    {...frame("historical-tool", "tool_call_requested", {id: "old", name: "old_activity", args: {}}, 2), interaction_id: "older-turn"},
  ]});
  assert.deepEqual(guideCurrentToolActivity({state, frames, bindings: [binding]}), {name: "inspect_blueprint", status: "pending"});
  for (const bindings of [[], [{...binding, operation_started_at: "2026-10-04T08:00:00Z"}],
    [{...binding, operation_request_id: "b".repeat(64)}],
    [binding, {...binding, interaction_id: "ambiguous-turn"}]]) {
    assert.equal(guideCurrentToolActivity({state, frames, bindings}), null);
  }
  frames = ingestGuideTimeline(log, {frames: [frame("result", "tool_result_received",
    {id: "call", name: "inspect_blueprint", is_error: false, result: "private result stays in the transcript"}, 3)]});
  assert.deepEqual(guideCurrentToolActivity({state, frames, bindings: [binding]}), {name: "inspect_blueprint", status: "success"});
  frames = ingestGuideTimeline(log, {frames: [
    frame("next-tool", "tool_call_requested", {id: "next", name: "publish_guide_edit", args: {}}, 4),
    frame("next-result", "tool_result_received", {id: "next", name: "publish_guide_edit", is_error: true, result: "The edit failed."}, 5),
  ]});
  assert.deepEqual(guideCurrentToolActivity({state, frames, bindings: [binding]}), {name: "publish_guide_edit", status: "error"});
  assert.equal(guideCurrentToolActivity({state: {...state, operation: {...state.operation, status: "completed"}},
    frames, bindings: [binding]}), null);
});

test("guide activity does not infer tool completion from result text without native evidence", () => {
  for (const result of [
    {id: "call", result: "success"},
    {id: "call", is_error: false},
    {id: "call", is_error: true},
  ]) {
    const frames = ingestGuideTimeline(createGuideFrameLog(), {frames: [
      frame("start", "tool_call_requested", {id: "call", name: "inspect_blueprint", args: {}}, 1),
      frame("result", "tool_result_received", result, 2),
    ]});
    assert.deepEqual(guideCurrentToolActivity({state, frames, bindings: [binding]}),
      {name: "inspect_blueprint", status: "pending"});
  }
});

test("a late native admission binding recovers skipped first frames exactly once", () => {
  const seen = new Set();
  const log = createGuideFrameLog();
  const older = {interaction_id: "older-turn", cursor: "console:1"};
  // The native cursor moves while only the older setup interaction is bound.
  let next = nextGuideTimelineCursor(undefined, {
    bindings: [older], next_cursor: "console:12", exhausted: true,
  }, seen);
  assert.deepEqual(next, {after: "console:12", replay: false});
  // Native admission at cursor10 was filtered by that previous binding snapshot.
  const late = {...binding, cursor: "console:10"};
  next = nextGuideTimelineCursor(next.after, {
    bindings: [older, late], frames: [], next_cursor: "console:15", exhausted: true,
  }, seen);
  assert.deepEqual(next, {after: "console:9", replay: true});
  const replay = {bindings: [older, late], next_cursor: "console:15", exhausted: true,
    frames: [{...frame("input", "user_input", {content: "Help my team"}), cursor: "console:10"},
      {...frame("reason", "reasoning_delta", {delta: "Recover this native reasoning."}, 1), cursor: "console:11"}]};
  let frames = ingestGuideTimeline(log, replay);
  next = nextGuideTimelineCursor(next.after, replay, seen);
  assert.deepEqual(next, {after: "console:15", replay: false});
  // Repeated native pages retain one input and one native reasoning entry.
  frames = ingestGuideTimeline(log, replay);
  const result = guideConversation({state, frames, bindings: [late]});
  assert.equal(result.entries.filter((entry) => entry.identity.role === "user").length, 1);
  assert.equal(allBlocks(result.entries).filter((block) => block.type === "thinking").length, 1);
  assert.equal(nextGuideTimelineCursor(next.after, replay, seen).replay, false);
});

test("newly discovered bindings rewind to the earliest missed admission but preserve forward progress", () => {
  const seen = new Set();
  const page = {bindings: [
    {interaction_id: "future", cursor: "console:101"},
    {interaction_id: "equal", cursor: "console:100"},
    {interaction_id: "earlier", cursor: "console:25"},
  ], next_cursor: "console:110"};
  assert.deepEqual(nextGuideTimelineCursor("console:100", page, seen), {after: "console:24", replay: true});
  assert.deepEqual(nextGuideTimelineCursor("console:24", page, seen), {after: "console:110", replay: false});
});

test("native streamed reasoning and paired tool results remain Console activity cards", () => {
  const frames = ingestGuideTimeline(createGuideFrameLog(), {frames: [
    frame("input", "user_input", {content: "Help my team"}),
    frame("thinking-1", "reasoning_delta", {delta: "Consider the independent contexts."}, 1),
    frame("thinking-2", "reasoning_complete", {text: "Consider the independent contexts."}, 2),
    frame("tool-1", "tool_call_requested", {id: "call-one", name: "mobkit_reference", args: {topic: "agents"}}, 3),
    frame("result-1", "tool_result_received", {id: "call-one", name: "mobkit_reference", result: "Durable domain agents preserve context."}, 4),
    frame("reply", "text_delta", {delta: "Start with one office agent."}, 5),
  ]});
  const conversation = guideConversation({state, frames, bindings: [binding]});
  const blocks = allBlocks(conversation.entries);
  assert.equal(blocks.filter((block) => block.type === "thinking").length, 1);
  assert.equal(blocks.find((block) => block.type === "thinking").text, "Consider the independent contexts.");
  assert.equal(blocks.filter((block) => block.type === "tool-call").length, 1);
  assert.equal(blocks.find((block) => block.type === "tool-call").name, "mobkit_reference");
  assert.ok(JSON.stringify(conversation.entries).includes("Start with one office agent."));
  assert.equal(conversation.phase, "generating");
});

test("current phase follows only the exactly bound attempt while every older activity remains in history", () => {
  const log = createGuideFrameLog();
  const current = {...state, operation: {...state.operation, started_at: "2026-10-04T09:05:00Z"}};
  const currentBinding = {...binding, interaction_id: "turn-two", user_message_id: "retry-user",
    operation_started_at: current.operation.started_at};
  let frames = ingestGuideTimeline(log, {frames: [
    {...frame("current-tool", "tool_call_requested", {id: "new-call", name: "inspect_blueprint", args: {}}, 1), interaction_id: "turn-two"},
    frame("old-reason", "reasoning_delta", {delta: "Earlier unfinished activity."}, 2),
  ]});
  let result = guideConversation({state: current, frames, bindings: [binding]});
  assert.equal(result.phase, "waiting", "Neither an older identical request nor an unbound current frame establishes progress");
  assert.ok(JSON.stringify(result.entries).includes("Earlier unfinished activity."));
  result = guideConversation({state: current, frames, bindings: [binding, currentBinding]});
  assert.equal(result.phase, "tool-executing", "Later old reasoning cannot replace the current tool phase");
  frames = ingestGuideTimeline(log, {frames: [
    {...frame("current-text", "text_delta", {delta: "The current reply."}, 3), interaction_id: "turn-two"},
    frame("old-tool", "tool_call_requested", {id: "old-call", name: "read_design_reference", args: {}}, 4),
  ]});
  result = guideConversation({state: current, frames, bindings: [binding, currentBinding]});
  assert.equal(result.phase, "generating", "A later historical tool cannot replace current streaming");
  assert.equal(allBlocks(result.entries).filter(block => block.type === "tool-call").length, 2);
  frames = ingestGuideTimeline(log, {frames: [
    {...frame("current-done", "interaction_complete", {}, 5), interaction_id: "turn-two"},
    frame("old-text", "text_delta", {delta: "Earlier late output."}, 6),
  ]});
  assert.equal(guideConversation({state: current, frames, bindings: [binding, currentBinding]}).phase, "waiting");
  assert.equal(guideConversation({state: {...current, operation: {...current.operation, status: "completed"}},
    frames, bindings: [binding, currentBinding]}).phase, null);
});

test("legacy, mismatched and ambiguous bindings preserve activity without claiming current progress", () => {
  const frames = ingestGuideTimeline(createGuideFrameLog(), {frames: [
    frame("tool", "tool_call_requested", {id: "call", name: "read_design_reference", args: {}}),
  ]});
  const legacy = {interaction_id: binding.interaction_id, user_message_id: binding.user_message_id, user_text: binding.user_text};
  for (const bindings of [[legacy], [{...binding, operation_request_id: "b".repeat(64)}],
    [{...binding, operation_started_at: "2026-10-04T08:00:00Z"}],
    [binding, {...binding, interaction_id: "ambiguous-turn"}]]) {
    const result = guideConversation({state, frames, bindings});
    assert.equal(result.phase, "waiting");
    assert.equal(allBlocks(result.entries).filter(block => block.type === "tool-call").length, 1);
  }
  assert.equal(guideConversation({state, frames, bindings: [binding, {...binding}]}).phase, "tool-executing",
    "Repeated pages with the same exact native interaction are not ambiguous");
});

test("native frame updates preserve the original entry and reject stale versions", () => {
  const log = createGuideFrameLog();
  const original = frame("tool", "tool_call_requested", {id: "call", name: "reference", args: {}}, 1);
  ingestGuideTimeline(log, {frames: [original]});
  const updated = {...original, status: "completed", frame_version: 2};
  let frames = ingestGuideTimeline(log, {frames: [
    frame("update", "frame_updated", {frame: updated}, 2),
  ]});
  assert.equal(frames.length, 1);
  assert.equal(frames[0].status, "completed");
  frames = ingestGuideTimeline(log, {frames: [original]});
  assert.equal(frames.length, 1);
  assert.equal(frames[0].status, "completed");
});

test("validated saved prose reconciles with native activity without duplicate user or assistant output", () => {
  const frames = ingestGuideTimeline(createGuideFrameLog(), {frames: [
    frame("input", "user_input", {content: "Help my team"}),
    frame("thinking", "reasoning_complete", {text: "Choose agents for distinct contexts."}, 1),
    frame("tool", "tool_call_requested", {id: "call", name: "publish_guide", args: {}}, 2),
    frame("reply", "text_delta", {delta: "INTERNAL_STRUCTURED_OUTPUT"}, 3),
  ]});
  const saved = {...state, operation: {status: "completed"}, messages: [
    {id: "saved-user", role: "user", text: "Help my team", created_at: new Date(1000).toISOString()},
    {id: "saved-assistant", role: "assistant", text: "An office agent owns office context.", created_at: new Date(1010).toISOString()},
  ]};
  const result = guideConversation({state: saved, frames,
    bindings: [{...binding, assistant_message_id: "saved-assistant"}],
  });
  assert.equal(result.entries.filter((entry) => entry.identity.role === "user").length, 1);
  assert.ok(!JSON.stringify(result.entries).includes("INTERNAL_STRUCTURED_OUTPUT"));
  assert.ok(JSON.stringify(result.entries).includes("An office agent owns office context."));
  assert.equal(allBlocks(result.entries).filter((block) => block.type === "thinking").length, 1);
  assert.equal(allBlocks(result.entries).filter((block) => block.type === "tool-call").length, 1);
  assert.equal(result.phase, null);
});

test("a repeated short answer remains visible until its own native interaction arrives", () => {
  const log = createGuideFrameLog();
  let frames = ingestGuideTimeline(log, {frames: [frame("old-input", "user_input", {content: "Yes"})]});
  const saved = {...state, messages: [
    {id: "saved-user", role: "user", text: "Yes"},
    {id: "saved-assistant", role: "assistant", text: "Which domain needs its own context?"},
  ]};
  const oldBinding = {...binding, user_text: "Yes", assistant_message_id: "saved-assistant"};
  const pendingMessage = {text: "Yes", role: "user", created_at: new Date(2000).toISOString()};
  let result = guideConversation({state: saved, frames, bindings: [oldBinding], pendingMessage});
  assert.ok(result.entries.some((entry) => entry.id === "setup-message:pending"));
  frames = ingestGuideTimeline(log, {frames: [{...frame("new-input", "user_input", {content: "Yes"}, 1000), interaction_id: "turn-two"}]});
  const retry = {...saved, operation: {status: "running", request_id: binding.operation_request_id,
    started_at: "2026-10-04T09:01:00Z"}};
  result = guideConversation({state: retry, frames,
    bindings: [oldBinding, {interaction_id: "turn-two", user_message_id: "new-user", user_text: "Yes",
      operation_request_id: retry.operation.request_id, operation_started_at: retry.operation.started_at}], pendingMessage});
  assert.ok(!result.entries.some((entry) => entry.id === "setup-message:pending"));
  assert.equal(result.entries.filter((entry) => entry.identity.role === "user").length, 2);
});

test("saved history survives unavailable native replay and genuine failures remain visible", () => {
  const saved = {...state, operation: {status: "failed"}, messages: [
    {id: "older", role: "user", text: "Preserve this old conversation"},
  ]};
  assert.ok(JSON.stringify(guideConversation({state: saved}).entries).includes("Preserve this old conversation"));
  const frames = ingestGuideTimeline(createGuideFrameLog(), {frames: [
    frame("failure", "interaction_failed", {error: "Run cancelled by operator"}),
  ]});
  assert.ok(JSON.stringify(guideConversation({state: saved, frames}).entries).includes("cancelled"));
});

test("attached evidence renders through native Console and survives saved-only replay", () => {
  const attachments = [
    {id: "a", filename: "notes [draft].pdf", media_type: "application/pdf", url: "/api/guide/setup-one/attachments/a"},
    {id: "b", filename: "night.png", media_type: "image/png", url: "/api/guide/setup-one/attachments/b"},
  ];
  const message = {id: "saved-user", role: "user", text: "Use the notes", attachments};
  const saved = {...state, messages: [message]};
  const offline = guideConversation({state: saved});
  assert.equal(allBlocks(offline.entries).filter((block) => block.type === "image").length, 1);
  assert.equal(allBlocks(offline.entries).find((block) => block.type === "image").src, attachments[1].url);
  assert.ok(JSON.stringify(offline.entries).includes("/attachments/a"));
  const frames = ingestGuideTimeline(createGuideFrameLog(), {frames: [frame("image-input", "user_input", {content: [
    {type: "text", text: "Use the notes\n\n[notes.pdf](/api/guide/setup-one/attachments/a)"},
    {type: "image", media_type: "image/png", url: attachments[1].url, alt: "night.png"},
  ]})]});
  const bound = {...binding, user_text: message.text, attachments};
  const native = guideConversation({state: saved, frames, bindings: [bound], pendingMessage: {...message, id: undefined}});
  assert.equal(native.entries.filter((entry) => entry.identity.role === "user").length, 1);
  assert.equal(allBlocks(native.entries).filter((block) => block.type === "image").length, 1);
  assert.equal(allBlocks(native.entries).find((block) => block.type === "image").src, attachments[1].url);
  const different = guideConversation({state: saved, frames, bindings: [bound],
    pendingMessage: {...message, attachments: [{...attachments[0], id: "different"}]}});
  assert.ok(different.entries.some((entry) => entry.id === "setup-message:pending"));
});

const unavailable = { status: "unavailable", startup_retry_required: false, available: false };
const emptyTimeline = { frames: [], bindings: [], next_cursor: null, exhausted: true };
const clickHandler = node => node[Object.keys(node).find(key => key.startsWith("__reactProps$"))].onClick;

// Mount the shipping timeline owner and Console presentation. Only transport
// replies are synthetic; a status action cannot send an answer or start a team.
async function mountedStartup(verify, { initialStatus = unavailable, heldChecks = false, savedMessages = [] } = {}) {
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], held = new Map(), timelineReads = new Map();
  const selected = structuredClone({ ...state, messages: savedMessages,
    operation: { ...state.operation, request: { text: "Original answer", selected: ["original-choice"] } } });
  const before = structuredClone(selected), draft = "  A newer unsent thought\n\u00e5\u{1f9ed}  ";
  let current = selected, root, statusReads = 0;
  const response = value => ({ ok: true, json: async () => structuredClone(value) });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: async (url, options = {}) => {
      calls.push({ url, method: options.method || "GET", body: options.body });
      assert.equal(options.method || "GET", "GET", "The mounted status action can only read.");
      assert.equal(options.body, undefined, "No saved answer or draft is sent.");
      if (url === "/api/architect/status") {
        const read = ++statusReads;
        if (read === 1) return response(initialStatus);
        if (heldChecks) return new Promise(resolve => held.set(read, value => { held.delete(read); resolve(response(value)); }));
        return response(initialStatus);
      }
      const match = /^\/api\/guide\/([^/]+)\/timeline\?/.exec(url);
      assert.ok(match, `Unexpected fixture read: ${url}`);
      const count = (timelineReads.get(match[1]) || 0) + 1; timelineReads.set(match[1], count);
      if (match[1] === selected.id && count === 1) throw new Error("Synthetic timeline is unavailable.");
      return response(emptyTimeline);
    } };
  const previous = new Map(Object.keys(replacements).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  function Mounted({ guide }) {
    const snapshot = useGuideTimeline(guide);
    return React.createElement(React.Fragment, null,
      React.createElement("textarea", { id: "current-draft", defaultValue: draft }),
      React.createElement(NativeGuideChat, { state: guide, snapshot }));
  }
  const render = () => act(async () => root.render(React.createElement(Mounted, { guide: current })));
  const button = label => [...dom.window.document.querySelectorAll("button")].find(node => node.textContent === label);
  try {
    const { createRoot } = await import("react-dom/client");
    root = createRoot(dom.window.document.getElementById("root"));
    await render();
    await verify({ document: dom.window.document, calls, button,
      invoke: callback => act(async () => { callback(); }),
      async select(id) { current = { ...selected, id }; await render(); },
      async release(read, value) { assert.ok(held.has(read)); await act(async () => held.get(read)(value)); },
      async unmount() { await act(async () => root.unmount()); root = null; },
      assertPreserved() { assert.deepEqual(selected, before); assert.equal(dom.window.document.querySelector("textarea").value, draft); },
    });
    assert.deepEqual(selected, before);
    assert.ok(calls.every(call => call.method === "GET" && call.body === undefined));
  } finally {
    if (root) await act(async () => root.unmount());
    await act(async () => { for (const release of [...held.values()]) release(unavailable); });
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key];
    }
  }
}

test("mounted unavailable startup offers one read-only check without claiming an unconfirmed answer is saved", async () => {
  await mountedStartup(async ({ document, calls, button, assertPreserved }) => {
    assert.match(document.querySelector(".setup-error").textContent, /Setup team is unavailable/);
    assert.match(document.querySelector(".setup-error").textContent, /This does not send or resend an answer/);
    assert.doesNotMatch(document.querySelector(".setup-error").textContent, /retained|saved|Retry|Start/);
    assert.equal(document.querySelectorAll(".setup-error button").length, 1);
    assert.equal(button("Check setup team status").disabled, false);
    assert.deepEqual(calls.map(call => call.url), [`/api/guide/${state.id}/timeline?limit=500`, "/api/architect/status"]);
    assertPreserved();
  });
});

test("mounted unavailable check stays single-owner while pending and preserves saved request, history and newer writing", async () => {
  await mountedStartup(async ({ document, calls, button, invoke, release, assertPreserved }) => {
    const check = clickHandler(button("Check setup team status"));
    const history = document.querySelector(".cc-conversation-transcript").textContent;
    await invoke(check);
    assert.equal(button("Checking setup team status...").disabled, true);
    await invoke(check);
    await act(async () => button("Checking setup team status...").click());
    assert.equal(calls.filter(call => call.url === "/api/architect/status").length, 2);
    assert.equal(document.querySelector(".cc-conversation-transcript").textContent, history);
    assertPreserved();
    await release(2, { status: "ready", startup_retry_required: false, available: true });
    assert.equal(document.querySelector(".setup-error") === null, true);
    assert.equal(document.querySelector(".cc-conversation-transcript").textContent, history);
    assertPreserved();
  }, { heldChecks: true, savedMessages: [{ id: "saved-user", role: "user", text: "Retained original conversation" }] });
});

test("mounted old unavailable action and delayed result stay inert after switching away and returning", async () => {
  await mountedStartup(async ({ document, calls, button, invoke, select, release, assertPreserved }) => {
    const check = clickHandler(button("Check setup team status"));
    await invoke(check);
    await select("replacement-guide");
    await select(state.id);
    const before = structuredClone(calls);
    await invoke(check);
    assert.deepEqual(calls, before, "A callback from the disposed original guide cannot read status again.");
    await release(2, unavailable);
    assert.equal(document.querySelector(".setup-error") === null, true, "A late old result cannot replace the returned guide's live observation.");
    assertPreserved();
  }, { heldChecks: true });
});

test("mounted typed failed startup keeps its existing retry control and an unmounted unavailable action cannot read", async () => {
  await mountedStartup(async ({ document, button }) => {
    assert.equal(button("Retry setup team").disabled, false);
    assert.equal(button("Check setup team status").disabled, false);
    assert.match(document.querySelector(".setup-error").textContent, /Starting the setup team does not resend your answer/);
  }, { initialStatus: { status: "failed", startup_retry_required: true, available: true } });
  await mountedStartup(async ({ calls, button, invoke, unmount }) => {
    const check = clickHandler(button("Check setup team status"));
    await unmount();
    const before = structuredClone(calls);
    await invoke(check);
    assert.deepEqual(calls, before);
  });
});
