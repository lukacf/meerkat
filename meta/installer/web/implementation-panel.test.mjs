import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import React, { act } from "react";
import { renderToStaticMarkup } from "react-dom/server";

async function loadModule(path, options = {}) {
  const built = await build({ entryPoints: [fileURLToPath(new URL(path, import.meta.url))],
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent", ...options });
  return import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
}
const {
  implementationCanDownload, implementationIsCurrent, implementationIsRunning,
  implementationFiles, implementationChecks, implementationUnresolved,
  implementationProgress, implementationPackageUrl, implementationConversation,
  implementationCheckSummary, implementationAcceptance,
  implementationAction, createImplementationAdmissionGate,
  implementationTiming, implementationOverview, implementationReport, implementationActivityStatus,
  implementationSampleSummary,
} = await loadModule("implementation-panel.mjs");
const { createGuideFrameLog, ingestGuideTimeline } = await loadModule("native-guide-chat.mjs");
const presentationOptions = { loader: { ".css": "empty" },
  nodePaths: [fileURLToPath(new URL("node_modules", import.meta.url))],
  alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) } };
const { ImplementationNextActions } = await loadModule("implementation-panel.jsx", presentationOptions);
const { navigateSetupProgress, SetupTrialShortcut, ManualSetupExports, NativeTeamPackage, nativeTeamPackagePlan,
  SetupOperationFailure, guideResponseTarget, guideResponseSignal, guideStatusText, createGuideResponseFollow,
  guidePublicationKey, createGuidePublicationUnread, guidePublicationVisible, GuideResponseNotice,
  guideQuestionVisible, guideResponseScrollTop,
} = await loadModule("guided.jsx", presentationOptions);
const { RehearsalActivityBanner } = await loadModule("proposal-simulation.jsx", presentationOptions);
const { trialActivityStatus } = await loadModule("simulation-state.mjs");
const { setupProgress } = await loadModule("setup-progress.mjs");

async function pendingBuilderFeedback(edited, verify, { readiness, blocked = false, savedDetails = {} } = {}) {
  const require = createRequire(import.meta.url);
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const initial = "  Keep this comparison\n\u00e5\u{1f9ed}  ";
  const state = { id: "pending-builder-guide", version: 12, proposal: { target: "mac" } };
  if (readiness) state.readiness = readiness;
  const url = `/api/guide/${state.id}/implementation`;
  const feedbackStorageKey = `mobkit-implementation-feedback-v1:${state.id}`;
  dom.window.localStorage.setItem(feedbackStorageKey, initial);
  const saved = { guide_id: state.id, guide_version: state.version, build_id: "pending-builder-build",
    current: true, status: "failed", ready_to_package: false, request: initial.trim(), ...savedDetails };
  let resolvePost, posted = false;
  const post = new Promise((resolve) => { resolvePost = resolve; });
  const calls = [];
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, localStorage: dom.window.localStorage, IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    fetch: async (target, options = {}) => {
      calls.push({ target, method: options.method || "GET" });
      if (target === '/api/checker-setup') return {ok:true,json:async()=>({status:'ready',can_prepare:false,detail:'Synthetic ready checker'})};
      if (target === url && options.method === "POST") {
        assert.deepEqual(JSON.parse(options.body), { version: state.version, request: initial.trim(), previous_build_id: null });
        posted = true;
        return post;
      }
      if (target === url) return posted ? { ok: true, json: async () => ({ ...saved }) }
        : { ok: false, status: 404, json: async () => ({ detail: "No saved build." }) };
      assert.ok(target.startsWith(`${url}/timeline?`), `Unexpected fixture read: ${target}`);
      return { ok: true, json: async () => ({ build_id: saved.build_id, frames: [], has_more: false }) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const { ImplementationPanel } = await loadModule("implementation-panel.jsx", { ...presentationOptions,
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
    } }] });
  const root = createRoot(dom.window.document.getElementById("root"));
  try {
    await act(async () => root.render(React.createElement(ImplementationPanel,
      { state, plan: { project: { name: "Synthetic comparison" } }, Icon: () => null })));
    const textarea = dom.window.document.querySelector("textarea");
    assert.equal(textarea.value, initial);
    const button = [...dom.window.document.querySelectorAll("button")].find((item) => item.textContent.includes("Build application"));
    assert.equal(button.disabled, blocked);
    await act(async () => button.click());
    if (blocked) {
      assert.equal(posted, false);
      assert.equal(calls.filter((call) => call.method === "POST").length, 0);
      assert.equal(textarea.value, initial);
      await verify(dom.window.document);
      return;
    }
    assert.equal(posted, true);
    assert.equal(textarea.disabled, false);
    if (edited) {
      const valueSetter = Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, "value").set;
      for (const text of ["A newer comparison", initial]) await act(async () => {
        valueSetter.call(textarea, text);
        textarea.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
      });
      assert.equal(textarea.value, initial);
    }
    await act(async () => {
      resolvePost({ ok: true, json: async () => ({ ...saved, status: "queued" }) });
      await new Promise((resolve) => setTimeout(resolve, 10));
    });
    await verify(dom.window.document.querySelector("textarea"), dom.window.localStorage, feedbackStorageKey, initial);
    assert.equal(calls.filter((call) => call.method === "POST").length, 1);
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
}

test("builder admission preserves a newer A -> B -> A feedback edit and its reload value", async () => {
  await pendingBuilderFeedback(true, (textarea, storage, key, text) => {
    assert.equal(textarea.value, text);
    assert.equal(storage.getItem(key), text);
  });
});

test("builder admission clears the unchanged submitted feedback revision only", async () => {
  await pendingBuilderFeedback(false, (textarea, storage, key) => {
    assert.equal(textarea.value, "");
    assert.equal(storage.getItem(key), null);
  });
});

test("the actual builder panel keeps actions ahead of its folded exact summary and technical diagnostics", async () => {
  const summary = "Added editable cost fields and retained the earlier result. <script>plain text</script>\n".repeat(50);
  await pendingBuilderFeedback(false, async textarea => {
    const document = textarea.ownerDocument;
    const disclosure = document.querySelector(".implementation-builder-summary");
    const actions = document.querySelector(".implementation-actions");
    const diagnostics = document.querySelector(".implementation-report");
    assert.equal(disclosure.tagName, "DETAILS");
    assert.equal(disclosure.open, false);
    assert.equal(disclosure.firstElementChild.tagName, "SUMMARY");
    assert.equal(disclosure.firstElementChild.textContent, "Builder summary");
    assert.ok(actions.querySelector("button"), "The existing next action remains outside optional builder prose");
    assert.ok(actions.compareDocumentPosition(disclosure) & document.defaultView.Node.DOCUMENT_POSITION_FOLLOWING);
    assert.equal(disclosure.querySelector("p").textContent, summary);
    assert.equal(disclosure.querySelector("script") === null, true);
    disclosure.firstElementChild.focus();
    assert.equal(document.activeElement === disclosure.firstElementChild, true);
    await act(async () => disclosure.firstElementChild.click());
    assert.equal(disclosure.open, true);
    assert.equal(disclosure.querySelector("p").textContent, summary);
    await act(async () => disclosure.firstElementChild.click());
    assert.equal(disclosure.open, false);
    assert.equal(diagnostics.open, false);
    assert.ok(disclosure.compareDocumentPosition(diagnostics) & document.defaultView.Node.DOCUMENT_POSITION_FOLLOWING);
    assert.doesNotMatch(diagnostics.textContent, /Added editable cost fields/);
    assert.match(diagnostics.textContent, /A saved build diagnostic/);
  }, { savedDetails: { summary, detail: "A saved build diagnostic" } });
});

test("waiting presentation shows the actual saved runtime startup step before native writing activity", async () => {
  const at = "2026-10-08T12:34:56.000Z";
  const message = "Starting the native MobKit implementation agent.";
  await pendingBuilderFeedback(false, textarea => {
    const document = textarea.ownerDocument;
    const notice = document.querySelector(".implementation-host-progress");
    assert.ok(notice, "The ordinary waiting view must expose the saved host startup step");
    assert.equal(notice.closest("details") === null, true, "Startup status must not require expanding diagnostics or progress");
    assert.match(notice.textContent, /Last saved host step/);
    assert.ok(notice.textContent.includes(message));
    assert.equal(notice.querySelector("time").dateTime, at);
    assert.equal(notice.getAttribute("role"), "status");
    const intro = document.querySelector(".implementation-panel > h2").nextElementSibling;
    assert.doesNotMatch(intro.textContent, /writing and testing/);
    assert.match(document.querySelector(".implementation-native").textContent, /Waiting for the builder/);
    assert.ok(notice.compareDocumentPosition(document.querySelector(".implementation-native")) & 4);
    assert.doesNotMatch(notice.textContent, /\b\d+%|ETA|time remaining/i);
  }, { savedDetails: { status: "running", phase: "implementation", detail: message,
    progress: [{ at, message }] } });
});

test("a host-declared necessary setup decision blocks builder admission while preserving its draft", async () => {
  await pendingBuilderFeedback(false, document => {
    assert.match(document.body.textContent, /Answer the guide's open setup decisions before building/);
  }, { blocked: true, readiness: { context_ready: true, decision_blockers: ["Choose the authority boundary."],
    implementation_blockers: [] } });
});

test("proposed defaults and open implementation work remain explicitly buildable", async () => {
  await pendingBuilderFeedback(false, textarea => assert.equal(textarea.value, ""), {
    readiness: { ready: false, context_ready: true, decision_blockers: [], implementation_blockers: ["Build custom controls."] } });
});

test("retained builder activity shows the current checking and review phase, then removes it when the receipt ends", async () => {
  const require = createRequire(import.meta.url);
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const receipt = { build_id: "retained-build", status: "checking", phase: "final_checks", native_identity: "implementation:main",
    timing: { version: 1, build_id: "retained-build", scope: "host_build", overlap: true,
      elapsed_ms: 45123, finished: false, phases: [] } };
  const frames = [{ id: "retained-answer", kind: "text_delta", payload: { delta: "The saved application source is ready for its checks." },
    identity: "implementation:main", source: { kind: "runtime" }, frame_version: 1,
    interaction_id: "retained-native-turn", timestamp_ms: 1000, cursor: "console:1" }];
  const originalFrames = structuredClone(frames), calls = [];
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    fetch: async (target, options = {}) => {
      calls.push({ target, method: options.method || "GET" });
      assert.equal(options.method, undefined);
      assert.ok(target.startsWith("/api/guide/retained-guide/implementation/timeline?"));
      return { ok: true, json: async () => ({ build_id: receipt.build_id, frames: structuredClone(frames), has_more: false }) };
    } };
  const previous = new Map(Object.keys(replacements).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(replacements)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const { BuilderActivity } = await loadModule("implementation-panel.jsx", { ...presentationOptions, jsx: "automatic",
    plugins: [{ name: "shared-mounted-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({
        path: pathToFileURL(require.resolve(args.path)).href, external: true,
      }));
    } }] });
  const root = createRoot(dom.window.document.getElementById("root"));
  const render = async (next) => act(async () => root.render(React.createElement(BuilderActivity,
    { guideId: "retained-guide", receipt: next, refresh: () => assert.fail("No timeline refresh was needed") })));
  const transcript = () => dom.window.document.querySelector('[data-native-implementation-chat="true"]');
  const notice = () => dom.window.document.querySelector(".implementation-current-phase");
  try {
    await render(receipt);
    const retainedTranscript = transcript();
    assert.match(retainedTranscript.textContent, /The saved application source is ready for its checks\./);
    assert.match(notice().textContent, /Checking the application.*Recorded so far: 45\.1s/);
    assert.equal(notice().getAttribute("role"), "status");
    await render({ ...receipt, status: "running", phase: "independent_review",
      timing: { ...receipt.timing, elapsed_ms: 70123 } });
    assert.match(notice().textContent, /Independent review in progress.*Recorded so far: 1m 10s/);
    assert.equal(transcript() === retainedTranscript, true);
    assert.match(transcript().textContent, /The saved application source is ready for its checks\./);
    for (const status of ["completed", "failed", "interrupted"]) {
      await render({ ...receipt, status, phase: "independent_review" });
      assert.equal(notice() === null, true, `The retained phase must not look active after ${status}`);
      assert.match(transcript().textContent, /The saved application source is ready for its checks\./);
    }
    assert.equal(calls.length, 1, "Receipt phase changes reuse the same native activity without a new timeline subscription");
    assert.deepEqual(frames, originalFrames);
  } finally {
    await act(async () => root.unmount());
    dom.window.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  }
});

const guide = { id: "observatory", version: 12 };
const checked = {
  build_id: "build-one", guide_id: guide.id, guide_version: guide.version,
  current: true, status: "completed", ready_to_package: true,
};

const navigationGuide = { ...guide, proposal: { target: "mac" } };
const navigationBuild = { ...checked, source_verified: true, source_digest: "navigation-source",
  acceptance_current: true, acceptance_trusted: true,
  acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "navigation-source", status: "pass", tests_run: 3 } };
const navigationFor = (progress, receipt = navigationBuild, state = navigationGuide) => progress && ({
  guide_id: state.id, guide_version: state.version, build_id: receipt.build_id,
  source_digest: receipt.source_digest, actions: progress.actions,
});
const renderNextActions = (props) => renderToStaticMarkup(React.createElement(ImplementationNextActions, {
  receipt: navigationBuild, state: navigationGuide, Icon: () => null, onNavigate: () => {}, ...props,
}));

const savedPublication = { id: "reading", version: 9, question: null,
  operation: { request_id: "completed-question", status: "completed", version: 8,
    started_at: "2026-10-01T16:51:38.499106+00:00", request: { action: "answer" } },
  messages: [{ id: "saved-user", role: "user", action: "answer", text: "Discuss the next step",
    created_at: "2026-10-01T16:51:38.510000+00:00" },
  { id: "a170b8e6-1f96-4326-9413-1e692c06a152", role: "assistant", action: "answer",
    text: "Your saved design is ready for the next step.", created_at: "2026-10-01T16:53:06.132925+00:00" }] };

test("New response requires an actual saved publication, not a failed attempt, native activity or metadata version change", () => {
  const before = structuredClone(savedPublication), key = guidePublicationKey(savedPublication);
  assert.equal(key, JSON.stringify([savedPublication.id, savedPublication.messages.at(-1).id]));
  assert.equal(guidePublicationKey({ ...savedPublication, version: 10, summary: "Different operational metadata" }), key);
  for (const status of ["running", "failed", "interrupted", undefined]) {
    assert.equal(guidePublicationKey({ ...savedPublication, operation: { ...savedPublication.operation, status } }), null);
  }
  for (const operation of [{ ...savedPublication.operation, started_at: "2026-10-01T17:00:00+00:00" },
    { ...savedPublication.operation, request: { action: "chat" } },
    { ...savedPublication.operation, version: 9 }, { ...savedPublication.operation, started_at: "invalid" }]) {
    assert.equal(guidePublicationKey({ ...savedPublication, operation }), null);
  }
  assert.equal(guidePublicationKey({ ...savedPublication, messages: [] }), null);
  assert.equal(guidePublicationKey({ ...savedPublication, messages: savedPublication.messages.slice(0, 1) }), null);
  assert.equal(guidePublicationKey({ ...savedPublication, messages: [{ ...savedPublication.messages.at(-1), text: " " }] }), null);
  assert.deepEqual(savedPublication, before);
});

test("first-use typing releases follow without fabricating unread response; only a new publication stays unread until its exact guide is viewed", () => {
  const unread = createGuidePublicationUnread(), follow = createGuideResponseFollow(), key = guidePublicationKey(savedPublication);
  assert.equal(unread.observe(savedPublication), key);
  assert.equal(unread.markSeen(savedPublication, key), true);
  follow.release(savedPublication); // The same production wheel/pointer/key release used by first-use fields.
  for (let input = 0; input < 4; input++) {
    follow.release(savedPublication);
    assert.equal(unread.observe({ ...savedPublication, version: 10 + input }), null);
  }
  const next = { ...savedPublication, version: 14,
    operation: { ...savedPublication.operation, version: 13, request_id: "next-question",
      started_at: "2026-10-01T17:01:00+00:00" },
    messages: [...savedPublication.messages, { ...savedPublication.messages.at(-1), id: "new-saved-assistant",
      created_at: "2026-10-01T17:01:10+00:00", text: "Here is the actual new response." }] };
  const nextKey = guidePublicationKey(next);
  assert.equal(unread.observe(next), nextKey);
  follow.release(next);
  assert.equal(unread.observe(next), nextKey);
  assert.equal(unread.markSeen(next, key), false, "A delayed reveal of the prior publication cannot consume the new one");
  const otherGuide = { ...next, id: "another-guide" };
  assert.equal(unread.markSeen(otherGuide, nextKey), false);
  assert.equal(unread.observe(otherGuide), guidePublicationKey(otherGuide));
  assert.equal(unread.markSeen(next, nextKey), true);
  assert.equal(unread.observe(next), null);
  assert.equal(unread.observe(otherGuide), guidePublicationKey(otherGuide), "Viewing one guide does not mark another guide seen");
  assert.equal(unread.observe({ ...next, operation: { ...next.operation, status: "failed" } }), null);
});

test("reaching the first-use footer or a stale guide target does not mark the saved response seen", () => {
  const key = guidePublicationKey(savedPublication), unread = createGuidePublicationUnread();
  const response = { dataset: { guideId: savedPublication.id, guidePublication: key } };
  let top = -2500;
  const assistant = { getBoundingClientRect: () => ({ top }) };
  const scroll = { scrollTop: 5000, scrollHeight: 5500, clientHeight: 500,
    getBoundingClientRect: () => ({ top: 100 }), querySelector: () => ({ querySelectorAll: () => [assistant] }) };
  const targets = { scroll, response };
  assert.equal(guidePublicationVisible(savedPublication, targets), false);
  assert.equal(unread.observe(savedPublication), key);
  const question = { getBoundingClientRect: () => ({ top: 116 }) };
  assert.equal(guidePublicationVisible(savedPublication, { ...targets, question }), false,
    "Seeing the choices below a long reply does not mean the reply itself was read");
  assert.equal(unread.observe(savedPublication), key);
  top = 116;
  for (const dataset of [{ ...response.dataset, guideId: "another-guide" },
    { ...response.dataset, guidePublication: "stale-response" }]) {
    assert.equal(guidePublicationVisible(savedPublication, { ...targets, response: { dataset } }), false);
  }
  assert.equal(guidePublicationVisible(savedPublication, { ...targets,
    scroll: { ...scroll, querySelector: () => null } }), false, "An empty transcript anchor is not the response");
  assert.equal(guidePublicationVisible(savedPublication, targets), true);
  unread.markSeen(savedPublication, key);
  top = 601;
  assert.equal(guidePublicationVisible(savedPublication, targets), false);
  assert.equal(unread.observe(savedPublication), null, "Scrolling away from a viewed response is not a new publication");
});

test("the production notice passes only its exact publication on explicit click and rejects stale or failed state", () => {
  const key = guidePublicationKey(savedPublication), observed = [], props = {
    state: savedPublication, publicationKey: key, busy: false,
    onReveal: (value) => observed.push(value), Icon: () => null };
  const element = GuideResponseNotice(props), html = renderToStaticMarkup(React.createElement(GuideResponseNotice, props));
  assert.match(html, /^<button type="button" class="setup-new-response">New response/);
  assert.doesNotMatch(html, /style=|dialog|first-use|saved design/);
  assert.deepEqual(observed, []);
  element.props.onClick();
  assert.deepEqual(observed, [key]);
  for (const changes of [{ publicationKey: null }, { publicationKey: "old-publication" }, { busy: true },
    { state: { ...savedPublication, id: "other-guide" } },
    { state: { ...savedPublication, operation: { ...savedPublication.operation, status: "failed" } } }]) {
    assert.equal(renderToStaticMarkup(React.createElement(GuideResponseNotice, { ...props, ...changes })), "");
  }
});

test("the notice occupies composer flow instead of floating over first-use content", async () => {
  const css = await readFile(new URL("guided.css", import.meta.url), "utf8");
  const rules = [...css.matchAll(/\.setup-new-response\s*\{([^}]+)\}/g)].map((match) => match[1]);
  assert.equal(rules.length, 1);
  assert.match(rules[0], /display:\s*flex/);
  assert.match(rules[0], /margin:\s*0 0 10px auto/);
  assert.doesNotMatch(rules[0], /position:\s*(absolute|fixed)|\b(bottom|left|transform|z-index):/);
});

test("the saved failed response is the production reveal target instead of the unchanged question or historical reply", () => {
  const requestId = "0e8ec6355c8c2cabff7c69934b7e20ca719f2c8be5406ee57943f83798bccd89";
  const observed = [], question = { getBoundingClientRect: () => ({ top: 200 }) }, response = {};
  const scroll = { clientHeight: 700, querySelectorAll: () => {
    observed.push("historical reply inspected");
    return [{ getBoundingClientRect: () => ({ top: 100 }) }];
  } };
  for (const status of ["failed", "interrupted"]) {
    const state = { id: "reading", version: 4, operation: { request_id: requestId, status,
      detail: "The guide could not finish this response. Your saved setup is preserved.", request: { text: "Review this setup" } } };
    const before = structuredClone(state), element = SetupOperationFailure({ state, busy: false, onRetry: () => {} });
    const failure = { dataset: { guideId: element.props["data-guide-id"],
      requestId: element.props["data-request-id"], operationStatus: element.props["data-operation-status"] } };
    assert.equal(guideResponseTarget(state, { scroll, question, response, failure, busy: false }), failure);
    for (const mismatch of [{ guideId: "another-setup" }, { requestId: "another-operation" }, { operationStatus: "running" }]) {
      assert.equal(guideResponseTarget(state, { scroll, question, response,
        failure: { dataset: { ...failure.dataset, ...mismatch } }, busy: false }), response);
    }
    assert.equal(guideStatusText({ state, busy: false, ready: true, review: true,
      question: { title: "Old implementation question" } }), state.operation.detail);
    assert.notEqual(guideResponseSignal(state, false, ""), guideResponseSignal({ ...state,
      operation: { ...state.operation, request_id: "replacement-operation" } }, false, ""));
    assert.notEqual(guideResponseSignal(state, false, ""), guideResponseSignal({ ...state,
      operation: { ...state.operation, status: "running" } }, false, ""));
    assert.deepEqual(state, before);
  }
  assert.deepEqual(observed, []);
});

test("the production saved-failure card retains the exact receipt and retry callback without admitting or consuming input", () => {
  const state = { id: "reading", version: 4, operation: { request_id: "exact-operation", status: "failed",
    detail: "The guide could not finish this response. <Private setup stays saved>",
    request: { text: "Review the saved setup without sending my unsent draft" } } };
  const input = { draft: "My unsent draft", selected: ["same-question-choice"], mode: "chat" }, before = structuredClone({ state, input });
  let retries = 0;
  const props = { state, busy: false, onRetry: () => { retries++; } }, element = SetupOperationFailure(props);
  const html = renderToStaticMarkup(React.createElement(SetupOperationFailure, props));
  assert.match(html, /role="alert"[^>]*tabindex="-1"/);
  assert.match(html, /data-guide-id="reading" data-request-id="exact-operation" data-operation-status="failed"/);
  assert.match(html, /The guide could not finish this response\. &lt;Private setup stays saved&gt;/);
  assert.match(html, /Review the saved setup without sending my unsent draft/);
  assert.doesNotMatch(html, /My unsent draft|same-question-choice/);
  assert.equal(retries, 0);
  const buttons = (node) => !React.isValidElement(node) ? [] : node.type === "button" ? [node]
    : React.Children.toArray(node.props.children).flatMap(buttons);
  const retry = buttons(element).find((button) => button.props.children === "Retry saved answer");
  assert.ok(retry, "the actual saved-answer retry control remains available");
  retry.props.onClick();
  assert.equal(retries, 1);
  assert.deepEqual({ state, input }, before);
  for (const status of ["running", "completed", undefined]) {
    assert.equal(renderToStaticMarkup(React.createElement(SetupOperationFailure,
      { ...props, state: { ...state, operation: { ...state.operation, status } } })), "");
  }
  assert.equal(renderToStaticMarkup(React.createElement(SetupOperationFailure, { ...props, busy: true })), "");
});

test("normal response targets and transient error announcements remain separate from durable operation failure", () => {
  const assistant = { getBoundingClientRect: () => ({ top: 100 }) }, question = { getBoundingClientRect: () => ({ top: 150 }) };
  const response = {}, scroll = { clientHeight: 700, querySelector: () => ({ querySelectorAll: () => [assistant] }) };
  const state = { id: "reading", version: 4, operation: { request_id: "active-operation", status: "completed" } };
  assert.equal(guideResponseTarget(state, { scroll, question, response, busy: false }), assistant);
  assert.equal(guideResponseTarget(state, { scroll, question: { getBoundingClientRect: () => ({ top: 1000 }) }, response, busy: false }), assistant);
  assert.equal(guideResponseTarget(state, { scroll: null, question: null, response, busy: false }), response);
  assert.equal(guideStatusText({ state, ready: true }), "Plan accepted. Download your agent team, or build a custom application.");
  assert.equal(guideStatusText({ state, question: { title: "Current question" } }), "Current question");
  const failed = { ...state, operation: { ...state.operation, status: "failed", detail: "Saved failure explanation" } };
  assert.equal(guideStatusText({ state: failed, error: "Connection interrupted" }), "The guide needs your attention. Your saved setup is preserved.");
  assert.equal(guideStatusText({ state: failed, error: "" }), "Saved failure explanation");
});

test("the exact revealed failure stays visible after native transcript restoration without jumping on unchanged polls", () => {
  const state = { id: "reading", version: 4, operation: { request_id: "same-failed-response", status: "failed" } };
  const before = structuredClone(state), observed = [], follow = createGuideResponseFollow();
  let failureOffset = 4124.375;
  const scroll = { scrollTop: 1593, clientHeight: 448, scrollHeight: 10297,
    getBoundingClientRect: () => ({ top: 0 }),
    scrollTo(options) { observed.push(options); this.scrollTop = options.top; } };
  const failure = { dataset: { guideId: state.id, requestId: state.operation.request_id,
    operationStatus: state.operation.status }, getBoundingClientRect: () => ({ top: failureOffset - scroll.scrollTop }) };
  assert.equal(follow.maintain(state, { scroll, failure }), false);
  assert.equal(follow.select(state, { failure }), true);
  assert.equal(follow.maintain(state, { scroll, failure }), true);
  assert.equal(failure.getBoundingClientRect().top, 16);
  assert.deepEqual(observed, [{ top: 4108.375, behavior: "auto" }]);
  assert.equal(follow.maintain(state, { scroll, failure }), false);
  failureOffset += 2500;
  scroll.scrollHeight += 2500;
  assert.equal(follow.maintain(state, { scroll, failure }), true);
  assert.equal(failure.getBoundingClientRect().top, 16);
  for (let poll = 0; poll < 4; poll++) assert.equal(follow.maintain(state, { scroll, failure }), false);
  assert.equal(observed.length, 2);
  assert.deepEqual(state, before);
});

test("manual history navigation releases failure following and fences delayed restoration until explicit reveal", () => {
  const state = { id: "reading", version: 4, operation: { request_id: "failed-response", status: "interrupted" } };
  let failureOffset = 1000;
  const observed = [], follow = createGuideResponseFollow(), scroll = { scrollTop: 0, scrollHeight: 4000, clientHeight: 500,
    getBoundingClientRect: () => ({ top: 0 }), scrollTo(options) { observed.push(options); this.scrollTop = options.top; } };
  const failure = { dataset: { guideId: state.id, requestId: state.operation.request_id, operationStatus: state.operation.status },
    getBoundingClientRect: () => ({ top: failureOffset - scroll.scrollTop }) };
  assert.equal(follow.release(state), true);
  assert.equal(follow.select(state, { failure }), false, "A queued automatic reveal must respect prior wheel/pointer navigation");
  assert.equal(follow.maintain(state, { scroll, failure }), false);
  assert.deepEqual(observed, []);
  assert.equal(follow.select(state, { failure }, true), true);
  follow.maintain(state, { scroll, failure });
  assert.equal(follow.release(state), true);
  scroll.scrollTop = 100;
  failureOffset += 1000;
  assert.equal(follow.maintain(state, { scroll, failure }), false);
  assert.equal(scroll.scrollTop, 100);
  assert.equal(follow.select(state, { failure }), false);
  assert.equal(follow.select(state, { failure }, true), true);
  assert.equal(follow.maintain(state, { scroll, failure }), true);
  assert.equal(failure.getBoundingClientRect().top, 16);
  assert.equal(observed.length, 2);
});

test("restoration following cannot borrow another guide, operation, status or stale failure element", () => {
  const state = { id: "reading", version: 4, operation: { request_id: "same-response", status: "failed" } };
  const follow = createGuideResponseFollow(), observed = [], scroll = { scrollTop: 0, scrollHeight: 4000, clientHeight: 500,
    getBoundingClientRect: () => ({ top: 0 }), scrollTo: (options) => observed.push(options) };
  const failure = { dataset: { guideId: state.id, requestId: state.operation.request_id, operationStatus: state.operation.status },
    getBoundingClientRect: () => ({ top: 1000 }) };
  follow.select(state, { failure });
  for (const other of [{ ...state, id: "another-guide" }, { ...state, version: 5 },
    { ...state, operation: { ...state.operation, request_id: "new-response" } },
    { ...state, operation: { ...state.operation, status: "interrupted" } },
    { ...state, operation: { ...state.operation, status: "running" } }]) {
    assert.equal(follow.maintain(other, { scroll, failure }), false);
  }
  assert.equal(follow.maintain(state, { scroll, failure: { ...failure, dataset: { ...failure.dataset, requestId: "stale" } } }), false);
  assert.deepEqual(observed, []);
});

test("a failed retry with the same request fingerprint follows its own new attempt rather than the released prior failure", () => {
  const prior = { id: "reading", version: 4, operation: { request_id: "same-idempotent-request", status: "failed",
    started_at: "2026-10-01T10:25:48.846739+00:00" } };
  const retried = { ...prior, operation: { ...prior.operation, started_at: "2026-10-01T10:45:02.183019+00:00" } };
  const card = (state) => {
    const element = SetupOperationFailure({ state, busy: false, onRetry: () => {} });
    return { dataset: { guideId: element.props["data-guide-id"], requestId: element.props["data-request-id"],
      operationStatus: element.props["data-operation-status"], operationStartedAt: element.props["data-operation-started-at"] },
      getBoundingClientRect: () => ({ top: 1000 }) };
  };
  const priorCard = card(prior), retryCard = card(retried), follow = createGuideResponseFollow(), observed = [];
  const scroll = { scrollTop: 0, scrollHeight: 4000, clientHeight: 500,
    getBoundingClientRect: () => ({ top: 0 }), scrollTo: (options) => observed.push(options) };
  assert.equal(follow.select(prior, { failure: priorCard }), true);
  follow.release(prior);
  assert.equal(follow.select(prior, { failure: priorCard }), false);
  assert.notEqual(guideResponseSignal(prior, false, ""), guideResponseSignal(retried, false, ""));
  assert.equal(follow.select(retried, { failure: priorCard }), false);
  assert.equal(follow.select(retried, { failure: retryCard }), true);
  assert.equal(follow.maintain(prior, { scroll, failure: priorCard }), false);
  assert.equal(follow.maintain(retried, { scroll, failure: priorCard }), false);
  assert.equal(follow.maintain(retried, { scroll, failure: retryCard }), true);
  assert.deepEqual(observed, [{ top: 984, behavior: "auto" }]);
});

test("response selection uses only the direct primary guide transcript when trial and reviewer transcripts are embedded", () => {
  const state = { id: "reading", version: 5, operation: { request_id: "published-response", status: "completed" } };
  const currentGuide = {}, oldGuide = {}, trialReply = {}, reviewerReply = {}, observed = [];
  const primary = { querySelectorAll(selector) {
    observed.push(["primary", selector]);
    return [oldGuide, currentGuide];
  } };
  const scroll = { clientHeight: 488,
    querySelector(selector) {
      assert.equal(selector, ':scope > [data-native-console-chat="true"]');
      return primary;
    },
    querySelectorAll() { return [oldGuide, currentGuide, trialReply, reviewerReply]; } };
  assert.equal(guideResponseTarget(state, { scroll, question: null, busy: false }), currentGuide);
  assert.deepEqual(observed, [["primary", ".cc-conversation-turn:last-child .cc-message--assistant"]]);
  const response = {};
  assert.equal(guideResponseTarget(state, { scroll: { ...scroll, querySelector: () => null },
    question: null, response, busy: false }), response);
});

test("a completed question-null guide response follows the restored primary reply rather than old embedded transcripts", async () => {
  const state = { id: "reading", version: 5, question: null, operation: { request_id: "same-published-response",
    status: "completed", started_at: "2026-10-01T10:52:01.123456+00:00" } };
  const before = structuredClone(state), follow = createGuideResponseFollow(), observed = [];
  let offset = 1969, reply = { getBoundingClientRect: () => ({ top: offset - scroll.scrollTop }) };
  const primary = { querySelectorAll: () => [reply] }, foreign = { getBoundingClientRect: () => ({ top: 9000 }) };
  const scroll = { scrollTop: 1953, clientHeight: 488, scrollHeight: 10297,
    querySelector: () => primary, querySelectorAll: () => [reply, foreign],
    getBoundingClientRect: () => ({ top: 0 }), scrollTo(options) { observed.push(options); this.scrollTop = options.top; } };
  const targets = { scroll, question: null };
  assert.equal(follow.select(state, targets), true);
  assert.equal(follow.maintain(state, targets), false);
  offset = 4513.47;
  reply = { getBoundingClientRect: () => ({ top: offset - scroll.scrollTop }) };
  assert.equal(follow.maintain(state, targets), true);
  assert.equal(scroll.scrollTop, 4497.47);
  assert.equal(reply.getBoundingClientRect().top, 16);
  for (let poll = 0; poll < 4; poll++) assert.equal(follow.maintain(state, targets), false);
  assert.equal(observed.length, 1);
  const source = await readFile(new URL("guided.jsx", import.meta.url), "utf8");
  const release = source.match(/  function releaseResponseFollow\(\) \{[\s\S]*?\n  \}/)?.[0];
  const reveal = source.match(/  const revealActionsTarget = \(target\) => \{[\s\S]*?\n  \};/)?.[0];
  assert.ok(release && reveal, "Exercise the production shared navigation and response release closures.");
  assert.match(source, /revealActionsTarget\(trialRef\.current\)/);
  assert.match(source, /revealActionsTarget\(implementationRef\.current\)/);
  assert.match(source, /revealActionsTarget\(deploymentRef\.current\)/);
  assert.match(source, /revealTarget: revealActionsTarget/);
  const frames = [], expanded = [], following = [], forceFollow = { current: true }, latest = { current: state };
  const navigate = new Function("state", "latestState", "responseFollow", "forceFollow", "setFollowing", "setActionsExpansion", "requestAnimationFrame",
    `${release}\n${reveal}\nreturn revealActionsTarget;`)(state, latest, { current: follow }, forceFollow,
    value => following.push(value), value => {
      assert.equal(follow.maintain(state, targets), false, "Following is released before expansion can resize the transcript.");
      expanded.push(value);
    }, callback => frames.push(callback));
  const lifecycleTarget = { isConnected: true, scrollIntoView(options) {
    assert.deepEqual(options, { behavior: "smooth", block: "start" });
    scroll.scrollTop = 400;
  } };
  navigate(lifecycleTarget);
  assert.deepEqual(expanded, [{ guideId: state.id, open: true }]);
  assert.equal(forceFollow.current, false);
  assert.deepEqual(following, [false]);
  frames.shift()();
  offset += 1000;
  assert.equal(follow.maintain(state, targets), false, "Transcript restoration cannot undo deliberate lifecycle navigation.");
  assert.equal(follow.select(state, targets), false, "Queued automatic reply reveal cannot reselect the released saved response.");
  assert.equal(scroll.scrollTop, 400);
  navigate(lifecycleTarget);
  latest.current = { ...state, id: "different-guide" };
  frames.shift()();
  assert.equal(scroll.scrollTop, 400, "A delayed navigation frame retains its original guide guard.");
  latest.current = state;
  navigate({ ...lifecycleTarget, isConnected: false });
  frames.shift()();
  assert.equal(scroll.scrollTop, 400, "Detached lifecycle targets cannot take the scroll.");
  assert.equal(follow.select(state, targets, true), true);
  assert.equal(follow.maintain(state, targets), true);
  assert.equal(reply.getBoundingClientRect().top, 16);
  assert.deepEqual(state, before);
});

test("a long completed guide reply stays the follow target when it restores above the current question", () => {
  const state = { id: "reading", version: 5, operation: { request_id: "question-response", status: "completed",
    started_at: "2026-10-01T10:52:01.123456+00:00" } };
  const follow = createGuideResponseFollow(), observed = [];
  let addedHeight = 0;
  const assistant = { getBoundingClientRect: () => ({ top: 100 + addedHeight - scroll.scrollTop }) };
  const question = { getBoundingClientRect: () => ({ top: 1600 + addedHeight - scroll.scrollTop }) };
  const scroll = { scrollTop: 1584, clientHeight: 488, scrollHeight: 5000,
    querySelector: () => ({ querySelectorAll: () => [assistant] }), getBoundingClientRect: () => ({ top: 0 }),
    scrollTo(options) { observed.push(options); this.scrollTop = options.top; } };
  const targets = { scroll, question };
  assert.equal(guideResponseTarget(state, { ...targets, busy: false }), assistant);
  assert.equal(follow.select(state, targets), true);
  addedHeight = 2000;
  scroll.scrollHeight += addedHeight;
  assert.equal(follow.maintain(state, targets), true);
  assert.equal(assistant.getBoundingClientRect().top, 16);
  assert.equal(question.getBoundingClientRect().top, 1516);
  assert.equal(follow.maintain(state, targets), false);
  assert.deepEqual(observed, [{ top: 2084, behavior: "auto" }]);
});

test("automatic long-answer arrival keeps the decision visible through restoration without claiming the reply was read", () => {
  const state = { ...savedPublication, question: { title: "Which context stays separate?" } };
  const key = guidePublicationKey(state), observed = [], follow = createGuideResponseFollow();
  let added = 0;
  const scroll = { scrollTop: 0, scrollHeight: 4000, clientHeight: 600, clientWidth: 700,
    getBoundingClientRect: () => ({ top: 100 }),
    querySelector: () => ({ querySelectorAll: () => [assistant] }),
    scrollTo(options) { observed.push(options); this.scrollTop = options.top; } };
  const assistant = { getBoundingClientRect: () => ({ top: 200 + added - scroll.scrollTop }) };
  const question = { dataset: { guideId: state.id, guideVersion: String(state.version) },
    getBoundingClientRect: () => ({ top: 1700 + added - scroll.scrollTop, height: 500 }),
    querySelector: () => ({ getBoundingClientRect: () => ({ bottom: 1970 + added - scroll.scrollTop, height: 100 }) }) };
  const response = { dataset: { guideId: state.id, guidePublication: key } };
  const targets = { scroll, question, response, preferQuestion: true };
  assert.equal(follow.select(state, targets), true);
  assert.equal(follow.maintain(state, targets), true);
  assert.equal(question.getBoundingClientRect().top, 260, "Desktop arrival retains response-tail context above the question");
  assert.equal(guideQuestionVisible(state, targets), true);
  assert.equal(guidePublicationVisible(state, targets), false, "Choices in view do not mark the off-screen native reply read");
  added = 1200;
  scroll.scrollHeight += added;
  assert.equal(follow.maintain(state, targets), true);
  assert.equal(guideQuestionVisible(state, targets), true);
  assert.equal(follow.maintain(state, targets), false, "Unchanged polls do not move the viewport");
  follow.release(state);
  scroll.scrollTop = 0;
  assert.equal(follow.maintain(state, targets), false, "Human history navigation releases decision following too");
  assert.equal(follow.select(state, targets, true), true);
  follow.maintain(state, targets);
  assert.equal(assistant.getBoundingClientRect().top, 116, "Explicit reading returns to the native reply opening");
  added += 300;
  scroll.scrollHeight += 300;
  follow.maintain(state, targets);
  assert.equal(assistant.getBoundingClientRect().top, 116, "Restoration retains explicit reading intent");
  assert.equal(guidePublicationVisible(state, targets), true);
});

test("decision arrival respects short answers, narrow viewports, stale questions, hidden panels and saved failure priority", () => {
  const state = { ...savedPublication, question: { title: "Current decision" } };
  const scroll = { scrollTop: 0, scrollHeight: 3000, clientHeight: 600, clientWidth: 700,
    getBoundingClientRect: () => ({ top: 0 }), querySelector: () => ({ querySelectorAll: () => [assistant] }) };
  const assistant = { getBoundingClientRect: () => ({ top: 100 }) };
  let questionTop = 220;
  const question = { dataset: { guideId: state.id, guideVersion: String(state.version) },
    getBoundingClientRect: () => ({ top: questionTop, height: 420 }),
    querySelector: () => ({ getBoundingClientRect: () => ({ bottom: questionTop + 250, height: 100 }) }) };
  const targets = { scroll, question, preferQuestion: true };
  assert.equal(guideResponseScrollTop(state, targets), 84, "Short answers stay at the native opening");
  assert.equal(guideQuestionVisible(state, targets), true);
  questionTop = 1600;
  assert.equal(guideResponseScrollTop(state, { ...targets, scroll: { ...scroll, clientWidth: 390 } }), 1584,
    "Narrow screens put the current decision near the top");
  for (const dataset of [{ ...question.dataset, guideId: "another-guide" },
    { ...question.dataset, guideVersion: String(state.version - 1) }]) {
    const stale = { ...targets, question: { ...question, dataset } };
    assert.equal(guideResponseScrollTop(state, stale), 84);
    assert.equal(guideQuestionVisible(state, stale), false);
  }
  assert.equal(guideResponseScrollTop(state, { ...targets, preferQuestion: false }), 84, "Actual lifecycle navigation retains reply priority");
  assert.equal(guideQuestionVisible(state, { ...targets, scroll: { ...scroll, clientWidth: 0 } }), false);
  assert.equal(guideQuestionVisible(state, { ...targets, question: { ...question, isConnected: false } }), false);
  const failed = { ...state, operation: { ...state.operation, status: "failed" } };
  const failure = { dataset: { guideId: state.id, requestId: failed.operation.request_id,
    operationStatus: "failed", operationStartedAt: failed.operation.started_at }, getBoundingClientRect: () => ({ top: 800 }) };
  assert.equal(guideResponseScrollTop(failed, { ...targets, failure }), 784);
});

test("the accepted production builder exposes the exact existing Try and Run actions without admitting work", () => {
  const progress = setupProgress(navigationGuide, { build: navigationBuild });
  const navigation = navigationFor(progress);
  const html = renderNextActions({ navigation });
  assert.match(html, /Try the application/);
  assert.match(html, /Set up and run/);
  assert.equal((html.match(/class="setup-action primary"/g) || []).length, 2);
  const installed = { guide_id: navigationGuide.id, deployment_id: "installed-navigation", build_id: navigationBuild.build_id,
    status: "running", process_live: true, health: "ready", console_url: "http://127.0.0.1:18789/console",
    bootstrap: { complete: true, ready: true, counts: { broken: 0 } } };
  const installedProgress = setupProgress(navigationGuide, { build: navigationBuild, deployment: installed });
  const installedNavigation = navigationFor(installedProgress);
  const installedHtml = renderNextActions({ navigation: installedNavigation });
  assert.match(installedHtml, /<button class="setup-action primary">Use your application/);
  assert.match(installedHtml, /<a class="setup-action" href="http:\/\/127.0.0.1:18789\/console"[^>]*>Administration Console/);
  assert.ok(installedHtml.indexOf("Use your application") < installedHtml.indexOf("Administration Console"));
  assert.match(renderNextActions({ navigation: { ...installedNavigation,
    actions: installedNavigation.actions.filter(action => action.target === "console") } }),
    /<a class="setup-action primary"/);
  const observed = [], targets = {
    trial: { current: { scrollIntoView: (options) => observed.push(["trial", options]) } },
    deployment: { current: { scrollIntoView: (options) => observed.push(["deployment", options]) } },
  };
  const onNavigate = (action) => navigateSetupProgress(action, progress, {
    onTrial: (mode) => observed.push(["mode", mode]), targets,
    onInstall: (kind) => {
      observed.push(["install-source", kind]);
      targets.deployment.current.scrollIntoView({ behavior: "smooth", block: "start" });
      return true;
    },
  });
  const element = ImplementationNextActions({ receipt: navigationBuild, state: navigationGuide,
    navigation, onNavigate, Icon: () => null });
  element.props.children[0].props.onClick();
  element.props.children[1].props.onClick();
  assert.deepEqual(observed, [["mode", "application"], ["trial", { behavior: "smooth", block: "start" }],
    ["install-source", "application"],
    ["deployment", { behavior: "smooth", block: "start" }]]);
  assert.deepEqual(navigation.actions, progress.actions);
});

test("builder next steps retain failed-trial review and active-trial follow instead of offering installation", () => {
  const trial = { rehearsal_id: "actual-trial", guide_id: guide.id, guide_version: guide.version,
    current: true, mode: "application", status: "completed",
    application_source: { build_id: navigationBuild.build_id, source_digest: navigationBuild.source_digest } };
  for (const problem of [{ status: "failed" }, { application_observations: [{ kind: "callback_failed" }] }]) {
    const progress = setupProgress(navigationGuide, { build: navigationBuild, trial: { ...trial, ...problem } });
    const html = renderNextActions({ navigation: navigationFor(progress) });
    assert.match(html, /Review trial/);
    assert.doesNotMatch(html, /Set up and run|Try the application/);
  }
  const progress = setupProgress(navigationGuide, { build: navigationBuild, trial: { ...trial, status: "running" } });
  const observed = [];
  const navigation = navigationFor(progress);
  assert.match(renderNextActions({ navigation }), /Follow trial/);
  navigateSetupProgress(navigation.actions[0], progress, {
    onTrial: () => observed.push("changed mode"),
    targets: { trial: { current: { scrollIntoView: () => observed.push("existing trial") } } },
  });
  assert.deepEqual(observed, ["existing trial"]);
});

test("builder next-step navigation is hidden for unaccepted, stale, mismatched or pending source", () => {
  const progress = setupProgress(navigationGuide, { build: navigationBuild });
  const navigation = navigationFor(progress);
  for (const change of [{ guide_id: "other" }, { guide_version: guide.version - 1 },
    { build_id: "earlier-build" }, { source_digest: "earlier-source" }]) {
    assert.equal(renderNextActions({ navigation: { ...navigation, ...change } }), "");
  }
  for (const receipt of [{ ...navigationBuild, current: false }, { ...navigationBuild, status: "running" },
    { ...navigationBuild, ready_to_package: false }]) {
    assert.equal(renderNextActions({ navigation, receipt }), "");
  }
  for (const options of [{ pendingChanges: true }, { designBusy: true }]) {
    assert.equal(renderNextActions({ navigation: navigationFor(setupProgress(navigationGuide,
      { build: navigationBuild }, options)) }), "");
  }
  const unaccepted = { ...navigationBuild, acceptance_trusted: false };
  assert.equal(renderNextActions({ receipt: unaccepted,
    navigation: navigationFor(setupProgress(navigationGuide, { build: unaccepted }), unaccepted) }), "");
  assert.equal(renderNextActions({ navigation, blocked: true }), "");
});

test("the shared navigation handler rejects replaced actions and preserves build and trial draft boundaries", () => {
  const progress = setupProgress(navigationGuide, { build: navigationBuild });
  const observed = [];
  const controls = { onTrial: (mode) => observed.push(mode),
    targets: { trial: { current: { scrollIntoView: () => observed.push("scroll") } } } };
  assert.equal(navigateSetupProgress({ ...progress.actions[0] }, progress, controls), false);
  assert.equal(navigateSetupProgress(progress.actions[0], null, controls), false);
  assert.equal(navigateSetupProgress(progress.actions[1], progress, controls), false);
  assert.deepEqual(observed, []);
  const installAction = progress.actions.find((action) => action.target === "deployment");
  assert.equal(installAction.sourceKind, "application");
  const installation = { targets: { deployment: { current: {
    scrollIntoView: () => observed.push("blind installation scroll"),
  } } }, revealTarget: () => observed.push("blind installation reveal") };
  assert.equal(navigateSetupProgress(installAction, progress, installation), false,
    "A source-specific installation needs its qualified navigation callback.");
  assert.equal(navigateSetupProgress(installAction, progress, {
    ...installation, onInstall: (kind) => { assert.equal(kind, "application"); return false; },
  }), false, "Rejected installation navigation cannot fall back to scrolling.");
  const replacement = setupProgress(navigationGuide, { build: navigationBuild });
  assert.equal(navigateSetupProgress(installAction, replacement, {
    ...installation, onInstall: () => assert.fail("An old action cannot select an installation source."),
  }), false);
  assert.deepEqual(observed, []);
  const buildAction = { target: "build", label: "Follow the build" };
  assert.equal(navigateSetupProgress(buildAction, { actions: [buildAction] }, {
    onTrial: () => observed.push("changed draft"), targets: { build: { current: { scrollIntoView: () => observed.push("builder") } } },
  }), true);
  assert.deepEqual(observed, ["builder"]);
});

test("the production Trial shortcut mirrors current application, failed-trial and live-session navigation", () => {
  const trial = { rehearsal_id: "owned-trial", guide_id: guide.id, guide_version: guide.version,
    current: true, mode: "application", application_source: {
      build_id: navigationBuild.build_id, source_digest: navigationBuild.source_digest } };
  const cases = [
    { label: "Try the application" },
    { label: "Review trial", trial: { ...trial, status: "completed", application_observations: [{ kind: "callback_failed" }] } },
    { label: "Continue trial", trial: { ...trial, status: "waiting_input", keep_open: true,
      can_continue: true, turn_count: 1, worker_stop_confirmed: false } },
    { label: "Follow trial", trial: { ...trial, status: "running" } },
    { label: "Follow trial", trial: { ...trial, status: "waiting_input", keep_open: true,
      can_continue: true, turn_count: 1, worker_stop_confirmed: false, guide_version: guide.version - 1 } },
  ];
  for (const item of cases) {
    const progress = setupProgress(navigationGuide, { build: navigationBuild, trial: item.trial });
    const action = progress.actions.find((value) => value.target === "trial"), observed = [];
    const props = { progress, onDesign: () => assert.fail("An existing progress trial must not select a fresh design"),
      onNavigate: (value) => {
        assert.equal(value, action);
        navigateSetupProgress(value, progress, { onTrial: (mode) => observed.push(mode),
          targets: { trial: { current: { scrollIntoView: () => observed.push("existing panel") } } } });
      } };
    assert.match(renderToStaticMarkup(React.createElement(SetupTrialShortcut, props)), new RegExp(item.label));
    SetupTrialShortcut(props).props.onClick({ preventDefault: () => observed.push("prevented link") });
    assert.deepEqual(observed, ["prevented link", ...(action.mode ? [action.mode] : []), "existing panel"]);
  }
});

test("the Trial shortcut explicitly labels its design fallback without inventing application eligibility", () => {
  for (const progress of [null, setupProgress(navigationGuide, { build: { ...navigationBuild, status: "running" } }),
    setupProgress(navigationGuide, { build: { ...navigationBuild, current: false } }),
    setupProgress(navigationGuide, { build: navigationBuild }, { pendingChanges: true }),
    setupProgress(navigationGuide, { build: navigationBuild }, { designBusy: true })]) {
    const observed = [], props = { progress, onNavigate: () => assert.fail("No trial action exists"),
      onDesign: () => observed.push("explicit design navigation") };
    const html = renderToStaticMarkup(React.createElement(SetupTrialShortcut, props));
    assert.match(html, />Try design</);
    assert.doesNotMatch(html, /Try the application|Continue trial/);
    SetupTrialShortcut(props).props.onClick({ preventDefault: () => {} });
    assert.deepEqual(observed, ["explicit design navigation"]);
  }
});

test("the actual simulation activity banner translates saved trial enums without changing native evidence", () => {
  const activity = { rehearsalId: "owned-trial", scope: "native-application-trial", status: "waiting_input",
    activeNodeIds: [], last: { identity: "lead:main", event: "interaction_complete", cursor: "console:42" } };
  const before = structuredClone(activity), names = new Map([["lead:main", "Conversation owner"]]);
  const render = (value) => renderToStaticMarkup(React.createElement(RehearsalActivityBanner, { activity: value, names }));
  const html = render(activity);
  assert.match(html, /Native application trial: waiting for your follow-up/);
  assert.doesNotMatch(html, /waiting_input/);
  assert.match(html, /Conversation owner: interaction complete/);
  assert.match(html, /proposal animation remains illustrative; external integrations are unavailable/);
  assert.deepEqual(activity, before);
  for (const [status, wording] of [["ending", "ending"], ["canceling", "stopping"],
    ["replaying", "replaying saved activity"], ["failed", "stopped with an error"], ["completed", "finished"]]) {
    assert.equal(trialActivityStatus(status), wording);
    assert.ok(render({ ...activity, status }).includes(`Native application trial: ${wording}`));
  }
  assert.match(render({ ...activity, status: "untrusted_internal_state" }), /Native application trial: saved activity/);
  assert.doesNotMatch(render({ ...activity, status: "untrusted_internal_state" }), /untrusted_internal_state/);
  assert.equal(render(null), "");
});

test("manual package actions stay optional without changing current application navigation or export callbacks", () => {
  const observed = [], plan = { project: { name: "Sample application", slug: "sample" }, target: "mac",
    agent_classes: [], connections: [], knowledge: { mode: "none" }, advanced: { port: 4110 } };
  const props = { plan, showPackage: true, checkedApplication: true, packageBusy: false,
    blocked: false, downloaded: false, saving: false, Icon: () => null,
    onPackage: (download) => observed.push(["package", download]), onBrief: () => observed.push("brief"),
    onSave: () => observed.push("save"), onAdjust: () => observed.push("adjust") };
  const html = renderToStaticMarkup(React.createElement(ManualSetupExports, props));
  assert.match(html, /^<details class="setup-manual-export"><summary>Manual exports and terminal setup \(optional\)<\/summary>/);
  assert.doesNotMatch(html, /<details[^>]*\bopen\b|setup-action primary|application is ready|ready to package/);
  assert.match(html, /Inspect your setup files, save a revision or download the implementation brief here\./);
  assert.doesNotMatch(html, /Download the implementation brief to keep your current plan for reference\./);
  assert.match(html, /Download checked application/);
  assert.match(html, /application-specific setup/);
  assert.match(html, /bash install\.sh/);
  assert.deepEqual(observed, []);
  const element = ManualSetupExports(props), packageCard = element.props.children[2];
  packageCard.props.children[3].props.onClick();
  packageCard.props.children[7].props.children[0].props.onClick();
  element.props.children[4].props.onClick();
  assert.deepEqual(observed, [["package", true], ["package", false], "brief"]);
  const progress = setupProgress(navigationGuide, { build: navigationBuild });
  assert.match(renderNextActions({ navigation: navigationFor(progress) }), /Try the application/);
  assert.match(renderNextActions({ navigation: navigationFor(progress) }), /Set up and run/);
});

test("native-only and reference-brief exports do not pretend to include checked application code", () => {
  const props = { plan: { project: { name: "Native team", slug: "native" }, target: "mac",
      agent_classes: [], connections: [], knowledge: { mode: "none" }, advanced: { port: 4110 } },
    showPackage: true, checkedApplication: false, blocked: true, saving: false, Icon: () => null,
    onPackage: () => assert.fail("Rendering must not export"), onBrief: () => assert.fail("Rendering must not export") };
  const native = renderToStaticMarkup(React.createElement(ManualSetupExports, props));
  assert.match(native, /NATIVE SETUP ONLY/);
  assert.match(native, /does not include the builder&#x27;s custom application code/);
  assert.match(native, /Download native setup only/);
  assert.match(native, /Inspect your setup files, save a revision or download the implementation brief here\./);
  assert.doesNotMatch(native, /Download checked application|Independent offline review passed/);
  const element = ManualSetupExports(props);
  assert.equal(element.props.children[2].props.children[3].props.disabled, true);
  assert.equal(element.props.children[4].props.disabled, true);
  const brief = renderToStaticMarkup(React.createElement(ManualSetupExports, { ...props, showPackage: false }));
  assert.match(brief, /Download the implementation brief to keep your current plan for reference\./);
  assert.match(brief, /Download implementation brief/);
  assert.doesNotMatch(brief, /Inspect your setup files|save a revision|Inspect native setup files|Save revision|Adjust details|bash install\.sh|Download native setup only/);
});

test("the optional team export preserves proposed defaults while keeping unfinished application work visible", () => {
  const plan = { project: { name: "Study team", slug: "study" }, target: "mac",
    agent_classes: [], connections: [], knowledge: { mode: "none" }, advanced: { port: 4110 } };
  const state = { id: "study", version: 2, status: "question", proposal: plan,
    readiness: { context_ready: true, decision_blockers: [], implementation_blockers: ["Check citation provenance."] } };
  const before = structuredClone(state), calls = [];
  const props = { state, blocked: false, Icon: () => null, onPackage: (...args) => calls.push(args) };
  const html = renderToStaticMarkup(React.createElement(NativeTeamPackage, props));
  assert.match(html, /^<details class="setup-manual-export"><summary>Agent team package \(optional\)<\/summary>/);
  assert.doesNotMatch(html, /^<details[^>]*\bopen(?:=|>)/);
  assert.match(html, /CURRENT PROPOSED PLAN/);
  assert.match(html, /Downloading does not accept the plan/);
  assert.match(html, /Download agent team package/);
  assert.match(html, /Check citation provenance/);
  assert.match(html, /bash install\.sh/);
  assert.doesNotMatch(html, /Download checked application|independent acceptance passed/i);
  const downloadButton = value => {
    const rendered = NativeTeamPackage(value);
    const section = rendered.type === "details" ? rendered.props.children[1] : rendered;
    return section.props.children.find(child => child?.type === "button" && child.props.onClick);
  };
  downloadButton(props).props.onClick();
  assert.deepEqual(calls, [[true, "native", state]]);
  assert.equal(nativeTeamPackagePlan(state), plan);
  assert.deepEqual(state, before);
  for (const changed of [{ id: null }, { version: undefined }, { operation: { status: "running" } },
    { readiness: { context_ready: true, decision_blockers: ["Choose an interface."] } },
    { readiness: { context_ready: false, decision_blockers: [] } }, { readiness: undefined }]) {
    assert.equal(nativeTeamPackagePlan({ ...state, ...changed }), null);
  }
  const accepted = { ...state, status: "ready", proposal: null, blueprint: plan };
  assert.equal(nativeTeamPackagePlan(accepted), plan);
  assert.match(renderToStaticMarkup(React.createElement(NativeTeamPackage, { ...props, state: accepted })), /ACCEPTED PLAN/);
  assert.equal(downloadButton({ ...props, blocked: true }).props.disabled, true);
  const agentOnly = { ...state, readiness: { ...state.readiness, implementation_blockers: [] } };
  const agentOnlyHtml = renderToStaticMarkup(React.createElement(NativeTeamPackage, { ...props, state: agentOnly }));
  assert.match(agentOnlyHtml, /^<section[^>]*aria-label="Agent team package"/);
  assert.doesNotMatch(agentOnlyHtml, /Agent team package \(optional\)/);
  assert.match(agentOnlyHtml, /Use your agent team/);
  assert.match(html, /does not implement remaining custom application work/);
});

test("the required application action reveals its builder without selecting a trial or starting work", () => {
  const state = { id: "study", version: 2, status: "refining", question: null,
    proposal: { project: { name: "Study team" }, target: "mac" },
    readiness: { context_ready: true, ready: false, decision_blockers: [], implementation_blockers: ["Build custom screens."] } };
  const progress = setupProgress(state), observed = [], builder = { scrollIntoView: () => assert.fail("Use the existing reveal handler") };
  const controls = { onTrial: () => assert.fail("Build navigation must preserve the trial selection"),
    targets: { build: { current: builder } }, revealTarget: target => observed.push(target) };
  assert.deepEqual(progress.actions, [{ target: "build", label: "Build application" }]);
  assert.equal(navigateSetupProgress(progress.actions[0], progress, controls), true);
  assert.deepEqual(observed, [builder]);
  assert.equal(navigateSetupProgress({ ...progress.actions[0] }, progress, controls), false);
  assert.equal(navigateSetupProgress(progress.actions[0], progress, { ...controls, targets: {} }), false);
  assert.deepEqual(observed, [builder]);
});

test("a long technical builder report stays exact without displacing a failed independent review's next action", () => {
  const summary = "Added startup-owned dispatch reservations and typed telemetry reconciliation. ".repeat(30);
  const detail = "Independent acceptance still needs attention after automatic repair.";
  const receipt = { ...checked, status: "failed", ready_to_package: false, source_digest: "source",
    summary, detail, checks: [{ name: "Authored tests", status: "pass" }], receipt: { tests_run: 39 },
    acceptance_current: true, acceptance_trusted: true,
    acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "source", status: "needs_repair", tests_run: 8,
      counterexamples: ["Requested behavior is missing."] } };
  const view = implementationOverview(receipt, guide);
  assert.equal(view.title, "Your application needs repair");
  assert.equal(view.attention, true);
  assert.match(view.next, /saved findings and regression tests/);
  assert.ok(!JSON.stringify(view).includes(summary));
  assert.ok(!JSON.stringify(view).includes("39"));
  assert.deepEqual(implementationReport(receipt), [
    { label: "Builder summary", text: summary }, { label: "Build diagnostic", text: detail }]);
  assert.equal(implementationCanDownload(receipt, guide), false);
});

test("plain-language status never treats builder claims or source checks as independent acceptance or live readiness", () => {
  const claimed = { ...checked, ready_to_package: false, summary: "Everything is deployed and ready!",
    checks: [{ name: "Source", status: "pass" }], receipt: { tests_run: 20 },
    acceptance: { issuer: "builder", status: "pass", tests_run: 20 } };
  assert.equal(implementationOverview(claimed, guide).title, "The saved source needs further verification");
  assert.equal(implementationReport(claimed)[0].text, "Everything is deployed and ready!");
  const accepted = { ...checked, source_digest: "exact-source", acceptance_current: true, acceptance_trusted: true,
    acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "exact-source", status: "pass", tests_run: 3 } };
  assert.equal(implementationOverview(accepted, guide).title, "Your application passed offline review");
  assert.match(implementationOverview(accepted, guide).next, /Live connections, custom interfaces and deployment still need/);
  for (const change of [{ acceptance_trusted: false }, { acceptance_current: false }, { source_digest: "changed-source" },
    { acceptance: { ...accepted.acceptance, tests_run: 0 } }])
    assert.notEqual(implementationOverview({ ...accepted, ...change }, guide).title, "Your application passed offline review");
});

test("current build phases, interruption and stale designs give the appropriate next step without rewriting evidence", () => {
  assert.equal(implementationOverview({ ...checked, status: "checking", phase: "independent_review" }, guide).title,
    "Independent review in progress");
  assert.equal(implementationOverview({ ...checked, status: "running", phase: "automatic_repair" }, guide).title,
    "Repairing review findings");
  assert.equal(implementationOverview({ ...checked, status: "interrupted" }, guide).title, "Your build was interrupted");
  assert.equal(implementationOverview({ ...checked, status: "failed" }, guide).title, "This build needs attention");
  const stale = implementationOverview({ ...checked, status: "running", current: false }, guide);
  assert.equal(stale.title, "An earlier proposal is still building");
  assert.match(stale.next, /Stop this build below before building/);
  const savedStale = implementationOverview({ ...checked, status: "completed", current: false }, guide);
  assert.equal(savedStale.title, "Build your updated design");
  assert.match(savedStale.next, /earlier source and reports remain available/);
  assert.equal(implementationOverview(null, guide).attention, false);
  assert.deepEqual(implementationReport({ summary: "  exact text\n", detail: "  exact text\n" }),
    [{ label: "Builder summary", text: "  exact text\n" }]);
});

test("sample availability is declared by exact current checked source and never invents a job or trial result", () => {
  const source = { ...checked, source_verified: true, source_digest: "exact-source",
    trial_samples: { source_digest: "exact-source", cases: [] } };
  assert.equal(implementationSampleSummary(source, guide).count, 0);
  assert.match(implementationSampleSummary(source, guide).message, /No prepared sample jobs/);
  const withJob = { ...source, trial_samples: { source_digest: "exact-source", cases: [
    { label: "Prepare tomorrow's brief", request: "Prepare tomorrow's brief using the sample notes.", fixtures: {} }] } };
  assert.equal(implementationSampleSummary(withJob, guide).count, 1);
  assert.match(implementationSampleSummary(withJob, guide).message, /offline data/);
  for (const change of [{ current: false }, { source_verified: false }, { source_digest: "changed-source" },
    { status: "running" }, { trial_samples: null }])
    assert.equal(implementationSampleSummary({ ...withJob, ...change }, guide), null);
  const before = JSON.stringify(withJob);
  implementationOverview(withJob, guide); implementationReport(withJob); implementationSampleSummary(withJob, guide);
  assert.equal(JSON.stringify(withJob), before);
});

test("timing disclosure binds the actual build and labels overlapping measurements without inferring provider time", () => {
  const timing = { version: 1, build_id: checked.build_id, scope: "host_build", overlap: true,
    elapsed_ms: 304187, finished: true, phases: [
      { phase: "builder_native", duration_ms: 210957, count: 1 },
      { phase: "tool", duration_ms: 12436, count: 14 },
      { phase: "private_unknown", label: "private raw content", duration_ms: 1, count: 1 },
    ] };
  const view = implementationTiming({ ...checked, timing });
  assert.equal(view.summary, "Recorded build time: 5m 4s");
  assert.deepEqual(view.phases, [
    { phase: "builder_native", label: "Builder native run", count: 1, duration: "3m 30s" },
    { phase: "tool", label: "Host tool callbacks", count: 14, duration: "12.4s" },
  ]);
  assert.equal(implementationTiming({ ...checked, timing: { ...timing, finished: false } }).summary, "Recorded so far: 5m 4s");
  assert.equal(implementationTiming({ ...checked, timing: { ...timing, partial: true } }).summary, "Partial measurement: 5m 4s");
  for (const change of [{ build_id: "old-build" }, { elapsed_ms: Infinity }, { elapsed_ms: "5 minutes" },
    { elapsed_ms: true }, { scope: "model" }, { overlap: false }, { version: 0 }])
    assert.equal(implementationTiming({ ...checked, timing: { ...timing, ...change } }), null);
  assert.equal(implementationTiming(checked), null);
});

test("ongoing activity uses receipt phases and only exact retained timing, without extending ended builds", () => {
  const receipt = { build_id: "current-build", status: "running", phase: "automatic_repair",
    timing: { version: 1, build_id: "another-build", scope: "host_build", overlap: true, elapsed_ms: 9000 } };
  assert.deepEqual(implementationActivityStatus(receipt), { label: "Repairing review findings", measurement: null });
  assert.deepEqual(implementationActivityStatus({ status: "queued" }), { label: "Waiting to build", measurement: null });
  for (const status of ["completed", "failed", "interrupted", "unknown", undefined])
    assert.equal(implementationActivityStatus({ ...receipt, status }), null);
  assert.equal(implementationActivityStatus(null), null);
});

test("trial repair and improvement admissions enter the builder once without replaying an older queued snapshot", () => {
  const gate = createImplementationAdmissionGate();
  const repair = { ...checked, build_id: "repair-build", status: "queued", ready_to_package: false };
  assert.equal(gate.consume(repair, guide), repair);
  // Polling has now advanced this build to independent review. Re-rendering
  // the retained admission, even as another object, cannot undo that progress.
  const polled = { ...repair, status: "checking", phase: "independent_review" };
  assert.equal(gate.consume({ ...repair }, guide), null);
  assert.equal(polled.status, "checking");
  const improvement = { ...repair, build_id: "improvement-build" };
  assert.equal(gate.consume(improvement, guide), improvement);
  assert.equal(gate.consume(repair, guide), null);
});

test("late trial admissions cannot enter another guide or revision, including navigation back to old context", () => {
  const gate = createImplementationAdmissionGate();
  assert.equal(gate.consume(checked, { id: "another-guide", version: 12 }), null);
  assert.equal(gate.consume(checked, guide), null);
  const changed = { ...checked, build_id: "stale-build", guide_version: 11 };
  assert.equal(gate.consume(changed, guide), null);
  assert.equal(gate.consume(changed, { ...guide, version: 11 }), null);
  const notCurrent = { ...checked, build_id: "not-current", current: false };
  assert.equal(gate.consume(notCurrent, guide), null);
  assert.equal(gate.consume({ ...notCurrent, current: true }, guide), null);
  const latest = { ...checked, build_id: "latest-build", guide_version: 13 };
  assert.equal(gate.consume(latest, { ...guide, version: 13 }), latest);
});

test("untyped admission values cannot reset receipt authority or suppress a later valid receipt", () => {
  const gate = createImplementationAdmissionGate();
  for (const receipt of [null, {}, { ...checked, guide_version: "12" },
    { ...checked, status: "ready" }, { ...checked, build_id: "" }])
    assert.equal(gate.consume(receipt, guide), null);
  assert.equal(gate.consume(checked, guide), checked);
});

test("failed independent behavior is a saved repair, while interruption resumes and stale source rebuilds", () => {
  const receipt = { ...checked, status: "failed", source_digest: "source",
    acceptance_trusted: true, acceptance_current: true,
    acceptance: { issuer: "mobkit-launch-acceptance-host", source_digest: "source", status: "needs_repair", tests_run: 4 } };
  assert.equal(implementationAction(receipt, guide), "Continue repairing application");
  assert.equal(implementationAction({ ...checked, status: "interrupted" }, guide), "Resume saved build");
  assert.equal(implementationAction({ ...receipt, current: false }, guide), "Build updated proposal");
  assert.equal(implementationAction(null, guide), "Build application");
});

test("independent review evidence cannot survive a source or guide change or model-only receipt", () => {
  const acceptance = { issuer: "mobkit-launch-acceptance-host", source_digest: "exact-source",
    status: "pass", tests_run: 4, requirements: [{ id: "observing", status: "tested" }] };
  const receipt = { ...checked, source_digest: "exact-source", acceptance,
    acceptance_trusted: true, acceptance_current: true };
  assert.equal(implementationAcceptance(receipt, guide).passed, true);
  for (const change of [{ acceptance_trusted: false }, { acceptance_current: false },
    { source_digest: "changed-source" }, { current: false }, { guide_version: 11 }])
    assert.equal(implementationAcceptance({ ...receipt, ...change }, guide), null);
  assert.equal(implementationAcceptance({ ...receipt, acceptance: { ...acceptance, issuer: "builder" } }, guide), null);
  assert.equal(implementationAcceptance({ ...receipt, acceptance: { ...acceptance, tests_run: 0 } }, guide).passed, false);
});

test("native reviewer activity is attributed separately from builder activity", () => {
  const frames = [
    { id: "builder-text", kind: "text_delta", payload: { delta: "Built the application." },
      source: { kind: "runtime" }, frame_version: 1, identity: "implementation:main",
      worker_native_identity: "implementation:main", interaction_id: "build-turn", timestamp_ms: 1000, cursor: "console:1" },
    { id: "review-text", kind: "text_delta", payload: { delta: "Found a cancellation counterexample." },
      source: { kind: "runtime" }, frame_version: 1, identity: "acceptance:main",
      worker_native_identity: "acceptance:main", interaction_id: "review-turn", timestamp_ms: 2000, cursor: "console:2" },
  ];
  const view = implementationConversation(checked, ingestGuideTimeline(createGuideFrameLog(), { frames }));
  assert.ok(view.entries.filter((entry) => entry.identity.role === "assistant").every((entry) => entry.identity.showLabel));
  const serialized = JSON.stringify(view.entries);
  assert.ok(serialized.includes("Application builder"));
  assert.ok(serialized.includes("Independent reviewer"));
  assert.ok(serialized.includes("Found a cancellation counterexample."));
});

test("package download requires explicit checked readiness for the exact current guide snapshot", () => {
  assert.equal(implementationCanDownload(checked, guide), true);
  for (const change of [
    { status: "checking" }, { status: "failed" }, { status: "interrupted" },
    { ready_to_package: false }, { ready_to_package: "true" },
    { current: false }, { current: undefined }, { guide_version: 11 },
    { guide_version: "12" }, { guide_id: "greenhouse" },
  ]) assert.equal(implementationCanDownload({ ...checked, ...change }, guide), false);
  assert.equal(implementationCanDownload(checked, { ...guide, version: 13 }), false);
  assert.equal(implementationCanDownload(null, guide), false);
});

test("an active build is not treated as terminal, while stale results remain inspectable", () => {
  for (const status of ["queued", "running", "checking"])
    assert.equal(implementationIsRunning({ status }), true);
  for (const status of ["completed", "failed", "interrupted", undefined])
    assert.equal(implementationIsRunning({ status }), false);
  const stale = { ...checked, current: false, files: [{ path: "app/main.py", size: 30 }] };
  assert.equal(implementationIsCurrent(stale, guide), false);
  assert.equal(implementationFiles(stale)[0].path, "app/main.py");
});

test("check failures and unavailable checks cannot become evidence of success", () => {
  const checks = [
    { name: "Syntax", status: "pass", detail: "Parsed application files" },
    { name: "Acceptance", status: "fail", detail: "Missing source adapter" },
    { name: "Live device", status: "unavailable", detail: "Device not connected" },
  ];
  assert.deepEqual(implementationChecks({ checks }), checks);
  assert.equal(implementationCanDownload({ ...checked, ready_to_package: false, checks }, guide), false);
  assert.deepEqual(implementationUnresolved({ unresolved: ["Device approval gate", "", null, "Device approval gate"] }), ["Device approval gate"]);
});

test("file paths and progress preserve backend evidence without rewriting or inventing it", () => {
  const files = [{ path: "../reported-invalid-path", size: 4, sha256: "evidence" }];
  const progress = [{ at: "2026-10-01T09:00:00Z", message: "Checking generated files" }];
  assert.deepEqual(implementationFiles({ files }), files);
  assert.notEqual(implementationFiles({ files })[0], files[0]);
  assert.deepEqual(implementationProgress({ progress }), progress);
  assert.deepEqual(implementationProgress({ progress: [{ percent: 80 }, null] }), []);
});

test("passing check disclosure summarizes real check and authored test counts without replacing exact evidence", () => {
  const checks = Array.from({ length: 20 }, (_, index) => ({ name: `check ${index}`, status: "pass", detail: `exact evidence ${index}` }));
  const receipt = { checks, receipt: { tests_run: 12 } };
  const summary = implementationCheckSummary(receipt);
  assert.equal(summary.label, "20 checks passed - 12 authored tests run");
  assert.equal(summary.allPassed, true);
  assert.equal(summary.attention, false);
  assert.deepEqual(implementationChecks(receipt), checks);
  assert.equal(implementationCheckSummary({ checks: checks.slice(0, 1), receipt: { tests_run: 1 } }).label, "1 check passed - 1 authored test run");
  assert.equal(implementationCheckSummary({ checks, receipt: { tests_run: "12" } }).label, "20 checks passed");
});

test("explicitly unverified current source labels passing checks as saved evidence", () => {
  const checks = Array.from({ length: 32 }, (_, index) => ({ name: `check ${index}`, status: "pass", detail: `saved evidence ${index}` }));
  const receipt = { checks, source_verified: false, source_digest: null, checked_digest: "earlier-source",
    receipt: { digest: "earlier-source", tests_run: 24 } };
  const summary = implementationCheckSummary(receipt);
  assert.equal(summary.saved, true);
  assert.equal(summary.attention, true);
  assert.equal(summary.allPassed, true);
  assert.equal(summary.passed, 32);
  assert.equal(summary.authoredTests, 24);
  assert.equal(summary.label, "Saved check results: 32 checks passed - 24 authored tests run");
  assert.deepEqual(implementationChecks(receipt), checks);
});

test("check freshness uses explicit source fields and preserves receipts without freshness information", () => {
  const checks = [{ name: "application tests", status: "pass" }];
  for (const binding of [
    { source_verified: true, source_digest: "current-source", checked_digest: "earlier-source" },
    { source_digest: "current-source", receipt: { digest: "earlier-source" } },
    { source_verified: true, source_digest: "current-source", checked_digest: "current-source", receipt: { digest: "earlier-source" } },
  ]) {
    const summary = implementationCheckSummary({ checks, ...binding });
    assert.equal(summary.saved, true);
    assert.equal(summary.attention, true);
    assert.match(summary.label, /^Saved check results:/);
  }
  for (const binding of [
    { source_verified: true, source_digest: "current-source", checked_digest: "current-source", receipt: { digest: "current-source" } },
    { status: "running", phase: "automatic_repair", current: false },
    { checked_digest: "saved-source", receipt: { digest: "saved-source" } },
  ]) {
    const summary = implementationCheckSummary({ checks, ...binding });
    assert.equal(summary.saved, false);
    assert.equal(summary.attention, false);
    assert.equal(summary.label, "1 check passed");
  }
});

test("actual builder check disclosure keeps saved rows without current-success styling", async () => {
  const checks = [{ name: "Exact authored test", status: "pass", detail: "Saved source evidence <script>inert</script>" }];
  for (const saved of [true, false]) {
    await pendingBuilderFeedback(false, textarea => {
      const document = textarea.ownerDocument;
      const disclosure = document.querySelector(".implementation-check-disclosure");
      assert.equal(disclosure.classList.contains("is-pass"), !saved);
      assert.equal(disclosure.classList.contains("needs-attention"), saved);
      assert.equal(disclosure.querySelector("summary").textContent,
        `${saved ? "Saved check results: " : ""}1 check passed - 1 authored test run`);
      const rows = disclosure.querySelectorAll(".implementation-checks > li");
      assert.equal(rows.length, 1);
      assert.equal(rows[0].classList.contains("is-pass"), !saved);
      assert.equal(rows[0].querySelector("strong").textContent, checks[0].name);
      assert.equal(rows[0].querySelector("small").textContent, saved ? "Previously passed" : "Passed");
      assert.equal(rows[0].querySelector("p").textContent, checks[0].detail);
      assert.equal(rows[0].querySelector("script") === null, true);
      const warning = [...document.querySelectorAll(".implementation-detail")]
        .find((item) => item.textContent === "Current source is not yet verified. These are saved check results.");
      assert.equal(!!warning, saved);
      if (saved) assert.equal(warning.closest("details") === null, true);
      assert.equal(disclosure.open, false);
    }, { savedDetails: { checks, source_verified: !saved, source_digest: saved ? null : "current-source",
      checked_digest: saved ? "earlier-source" : "current-source",
      receipt: { digest: saved ? "earlier-source" : "current-source", tests_run: 1 } } });
  }
});

test("failed, unavailable and unknown outcomes expand evidence and never look like all checks passed", () => {
  const checks = [{ name: "syntax", status: "pass" }, { name: "acceptance", status: "fail" },
    { name: "live account", status: "unavailable" }, { name: "unknown", status: "pending" }];
  const summary = implementationCheckSummary({ checks, receipt: { tests_run: 3 } });
  assert.equal(summary.allPassed, false);
  assert.equal(summary.attention, true);
  assert.equal(summary.label, "1 passed, 1 failed, 2 not verified - 3 authored tests run");
  assert.equal(implementationCheckSummary({ checks: [] }).allPassed, false);
  assert.equal(implementationCheckSummary({ checks: [{ name: "tests", status: "pass" }], receipt: { tests_run: -1 } }).authoredTests, null);
});

test("primary application download is pinned to its checked guide and build, including open guide requirements", () => {
  const incompleteGuide = { ...guide, status: "handoff", outstanding_requirements: ["Connect the live telescope"] };
  assert.equal(implementationPackageUrl(checked, incompleteGuide), "/api/guide/observatory/implementation/package?build_id=build-one");
  assert.equal(implementationPackageUrl({ ...checked, build_id: "build two" }, guide), "/api/guide/observatory/implementation/package?build_id=build+two");
  assert.equal(implementationPackageUrl(checked, { ...guide, version: 13 }), null);
  assert.equal(implementationPackageUrl({ ...checked, ready_to_package: false }, guide), null);
  assert.equal(implementationPackageUrl({ ...checked, guide_id: "another setup" }, guide), null);
});

test("implementation Console retains real thinking, tools, results and streamed prose while hiding launch scaffolding", () => {
  const frame = (id, kind, payload, offset) => ({ id, kind, payload,
    interaction_id: "implementation-turn", identity: "implementation:main", source: { kind: "runtime" },
    timestamp_ms: 1000 + offset, cursor: `console:${offset + 1}`, frame_version: 1 });
  const log = createGuideFrameLog();
  const page = { frames: [
    frame("launch", "user_input", { content: "PRIVATE_LAUNCH_SCAFFOLD" }, 0),
    frame("reasoning", "reasoning_complete", { text: "Check cancellation before retrying the live operation." }, 1),
    frame("call", "tool_call_requested", { id: "check-one", name: "run_checks", args: { scope: "application" } }, 2),
    frame("result", "tool_result_received", { id: "check-one", name: "run_checks", result: "Cancellation acceptance test passed." }, 3),
    frame("prose", "text_delta", { delta: "The application passes its cancellation test." }, 4),
  ] };
  let conversation = implementationConversation({ ...checked, native_identity: "implementation:main" }, ingestGuideTimeline(log, page));
  const serialized = JSON.stringify(conversation.entries);
  assert.ok(!serialized.includes("PRIVATE_LAUNCH_SCAFFOLD"));
  assert.ok(serialized.includes("The application passes its cancellation test."));
  const blocks = conversation.entries.flatMap((entry) => entry.blocks || []);
  assert.equal(blocks.filter((block) => block.type === "thinking").length, 1);
  assert.equal(blocks.find((block) => block.type === "tool-call").name, "run_checks");
  assert.ok(serialized.includes("Cancellation acceptance test passed."));
  assert.equal(conversation.viewState.conversationId, "implementation:build-one");
  conversation = implementationConversation(checked, ingestGuideTimeline(log, page));
  assert.equal(conversation.entries.flatMap((entry) => entry.blocks || []).filter((block) => block.type === "tool-call").length, 1);
});


test("the agent-only progress action reveals its actual package card without admitting a build or download", () => {
  const state = { id: "team", version: 2, status: "question", proposal: { project: { name: "Team" } },
    readiness: { context_ready: true, decision_blockers: [], implementation_blockers: [] } };
  const progress = setupProgress(state), observed = [];
  const controls = { onTrial: () => assert.fail("Package navigation is not a trial"),
    targets: { package: { current: { scrollIntoView: options => observed.push(options) } } } };
  assert.equal(navigateSetupProgress(progress.actions[0], progress, controls), true);
  assert.deepEqual(observed, [{ behavior: "smooth", block: "start" }]);
  assert.equal(navigateSetupProgress({ ...progress.actions[0] }, progress, controls), false);
  assert.equal(navigateSetupProgress(progress.actions[0], null, controls), false);
});
