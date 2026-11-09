import { test } from "node:test";
import assert from "node:assert/strict";
import { build as bundle } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";
import React, { act } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { applicationTrialReplayOffer, applicationTrialReplayResult, createTrialReplayStore, trialReplayAcknowledged,
  trialReplayRejected, requestTrialReplay } from "./trial-replay.mjs";
import { createRehearsalDraftStore, rehearsalSetupDraftOwner, rehearsalTurnDraftOwner } from "./rehearsal-drafts.mjs";

const guide = { id: "67f2f55d-8fd4-4d92-b999-14fb240e5730", version: 9 };
const oldTrial = "073bd98c-35c2-4fae-a95a-b96c4cf67515";
const originalBuild = "ddfc23ac-f3cf-4cc3-9fb4-bf71a8883b9d";
const targetBuild = "c29b7357-7991-4a7f-9656-155a3e9b0fe8";
const actionId = "d8377671-eccc-4f39-937a-4108a5e43594";
const nextAction = "22443f55-c84a-420c-a5f2-f3f3d478fe9c";
const build = { guide_id: guide.id, guide_version: 9, current: true, status: "completed", source_verified: true,
  cleanup_pending: false, build_id: targetBuild, source_digest: "b".repeat(64),
  trial_replay: { available: true, guide_id: guide.id, guide_version: 9, rehearsal_id: oldTrial,
    original_build_id: originalBuild, original_source_digest: "a".repeat(64), build_id: targetBuild,
    source_digest: "b".repeat(64), payload_sha256: "c".repeat(64), detail: "The host checked the saved job." } };
const oldReceipt = { guide_id: guide.id, guide_version: 9, rehearsal_id: oldTrial, current: false,
  mode: "application", status: "completed", worker_stop_confirmed: true, application_stop_confirmed: true,
  application_source: { build_id: originalBuild, source_digest: "a".repeat(64) } };
const offer = () => applicationTrialReplayOffer(build, guide);
const key = "mobkit-selected-job-replay-v1:" + guide.id;
const storage = () => {
  const values = new Map();
  return { values, getItem: (name) => values.get(name) ?? null,
    setItem: (name, value) => values.set(name, value), removeItem: (name) => values.delete(name) };
};
function admitted(action, extra = {}) {
  return { guide_id: action.guide_id, guide_version: action.body.version, rehearsal_id: action.body.start_request_id,
    mode: "application", status: "queued", current: true, start_request_id: action.body.start_request_id,
    start_request_complete: true, start_request_fingerprint: "d".repeat(64),
    application_source: { build_id: action.body.build_id, source_digest: action.body.source_digest },
    trial_replay: { replay_version: 1, original_rehearsal_id: action.rehearsal_id,
      original_build_id: action.body.original_build_id, original_source_digest: action.body.original_source_digest,
      build_id: action.body.build_id, source_digest: action.body.source_digest, payload_sha256: action.payload_sha256 }, ...extra };
}
function rejected(action, changes = {}) {
  return { guide_id: action.guide_id, guide_version: action.body.version, mode: "application",
    start_request_id: action.body.start_request_id, start_request_fingerprint: "d".repeat(64),
    witness_version: 1, status: "rejected", start_request_complete: false, replay_version: 1,
    original_rehearsal_id: action.rehearsal_id, original_build_id: action.body.original_build_id,
    original_source_digest: action.body.original_source_digest, build_id: action.body.build_id,
    source_digest: action.body.source_digest, payload_sha256: action.payload_sha256, ...changes };
}
function completedReplay() {
  const action = createTrialReplayStore(storage()).begin(offer(), actionId);
  const result = admitted(action);
  return { ...result, status: "completed", worker_stop_confirmed: true,
    application_stop_confirmed: true, trace_complete: true,
    trial_replay: { ...result.trial_replay, status: "completed", expected_turns: 1, completed_turns: 1 } };
}

test("the repaired build's exact host offer is usable while the original trial remains historical", () => {
  assert.equal(oldReceipt.current, false);
  assert.equal(offer().rehearsal_id, oldTrial);
  for (const changes of [{ trial_replay: undefined }, { current: false }, { source_verified: false },
    { cleanup_pending: true }, { source_digest: "f".repeat(64) }, { status: "running" }])
    assert.equal(applicationTrialReplayOffer({ ...build, ...changes }, guide), null);
  for (const changes of [{ available: "true" }, { available: false }, { guide_version: 8 },
    { build_id: originalBuild }, { source_digest: "a".repeat(64) }, { payload_sha256: "invalid" },
    { rehearsal_id: "../history" }, { original_build_id: 12 }])
    assert.equal(applicationTrialReplayOffer({ ...build, trial_replay: { ...build.trial_replay, ...changes } }, guide), null);
  assert.equal(applicationTrialReplayOffer(build, { ...guide, version: 10 }), null);
});

test("the persisted retry contains IDs only and survives reload and later source changes unchanged", () => {
  const saved = storage(), store = createTrialReplayStore(saved), action = store.begin(offer(), actionId);
  assert.ok(action);
  assert.deepEqual(action.body, { version: 9, original_build_id: originalBuild, original_source_digest: "a".repeat(64),
    build_id: targetBuild, source_digest: "b".repeat(64), start_request_id: actionId });
  assert.doesNotMatch(JSON.stringify(action), /fixtures|text|token|request"/);
  action.body.build_id = originalBuild;
  assert.equal(createTrialReplayStore(saved).pending(guide.id).body.build_id, targetBuild);
  assert.equal(store.begin({ ...offer(), build_id: originalBuild }, nextAction), null);
  assert.equal(store.pending("073bd98c-35c2-4fae-a95a-b96c4cf67515"), null);
});

test("acknowledgment requires the action, both source versions, capsule hash and direct committed start", () => {
  const saved = storage(), store = createTrialReplayStore(saved), action = store.begin(offer(), actionId), result = admitted(action);
  for (const changes of [{ rehearsal_id: oldTrial }, { start_request_id: nextAction }, { start_request_complete: false },
    { guide_id: oldTrial }, { guide_version: 8 }, { mode: "design" }, { start_request_fingerprint: null },
    { application_source: { build_id: originalBuild, source_digest: "a".repeat(64) } }]) {
    assert.equal(trialReplayAcknowledged(action, { ...result, ...changes }), false);
    assert.equal(store.acknowledge(guide.id, { ...result, ...changes }), false);
  }
  for (const field of ["replay_version", "original_rehearsal_id", "original_build_id", "original_source_digest", "build_id", "source_digest", "payload_sha256"])
    assert.equal(trialReplayAcknowledged(action, { ...result, trial_replay: { ...result.trial_replay, [field]: "wrong" } }), false);
  // Recovery can confirm a prior admission even if later source edits make it historical.
  assert.equal(store.acknowledge(guide.id, { ...result, current: false }), true);
  assert.equal(store.pending(guide.id), null);
});

test("generic failures and unbound negative witnesses cannot unlock a separate retry", () => {
  const saved = storage(), store = createTrialReplayStore(saved), action = store.begin(offer(), actionId);
  for (const changes of [{ payload_sha256: "f".repeat(64) }, { original_rehearsal_id: nextAction },
    { original_source_digest: "f".repeat(64) }, { build_id: originalBuild }, { guide_version: 8 },
    { start_request_id: nextAction }, { replay_version: 0 }, { status: "failed" }, { extra: "private" }]) {
    assert.equal(trialReplayRejected(action, rejected(action, changes)), null);
    assert.equal(store.recordRejected(guide.id, rejected(action, changes)), false);
    assert.equal(store.keepRejected(guide.id, actionId), false);
  }
  assert.equal(store.recordRejected(guide.id, rejected(action)), true);
  assert.equal(store.keepRejected(guide.id, nextAction), false);
  assert.equal(store.keepRejected(guide.id, actionId), true);
  assert.equal(store.pending(guide.id), null);
  const next = store.begin(offer(), nextAction);
  assert.equal(next.body.start_request_id, nextAction);
  assert.equal(JSON.parse(saved.getItem(key + ":rejected"))[0].body.start_request_id, actionId);
});

test("unavailable, silent or corrupted browser storage prevents sending or forgetting the retry", () => {
  for (const denied of [null, { getItem() { throw new Error("denied"); }, setItem() {} },
    { getItem() { return null; }, setItem() {} }])
    assert.equal(createTrialReplayStore(denied).begin(offer(), actionId), null);
  for (const value of ["not-json", "x".repeat(8001), JSON.stringify({ guide_id: guide.id, body: {} })]) {
    const saved = storage(); saved.setItem(key, value);
    const store = createTrialReplayStore(saved);
    assert.equal(store.pending(guide.id), null); assert.equal(store.blocked(guide.id), true);
    assert.equal(store.begin(offer(), actionId), null);
  }
  const saved = storage(), action = createTrialReplayStore(saved).begin(offer(), actionId);
  saved.removeItem = () => {};
  const store = createTrialReplayStore(saved);
  assert.equal(store.acknowledge(guide.id, admitted(action)), false);
  assert.deepEqual(store.pending(guide.id), action);
  saved.setItem = () => {};
  assert.equal(store.recordRejected(guide.id, rejected(action)), false);
  assert.deepEqual(store.pending(guide.id), action);
});

test("replay transport preserves HTTP ambiguity and exact serialized action on recovery", async () => {
  const saved = storage(), action = createTrialReplayStore(saved).begin(offer(), actionId), calls = [];
  const fetcher = async (url, request) => { calls.push({ url, body: request.body });
    if (calls.length === 1) throw new Error("Lost reply");
    return { ok: true, json: async () => admitted(action) }; };
  await assert.rejects(requestTrialReplay("/same/replay", action.body, fetcher), /Lost reply/);
  const restored = createTrialReplayStore(saved).pending(guide.id);
  assert.equal(trialReplayAcknowledged(restored, await requestTrialReplay("/same/replay", restored.body, fetcher)), true);
  assert.deepEqual(calls[0], calls[1]);
  for (const status of [404, 409, 422, 503, 504]) {
    const response = { ok: false, status, json: async () => ({ detail: "Unconfirmed failure" }) };
    await assert.rejects(requestTrialReplay("/same/replay", action.body, async () => response), /Unconfirmed/);
  }
  await assert.rejects(requestTrialReplay("/same/replay", action.body,
    async () => ({ ok: true, json() { throw new Error("Malformed"); } })), /could not be read/);
  const negative = await requestTrialReplay("/same/replay", action.body,
    async () => ({ ok: false, status: 409, json: async () => ({ detail: "No admission", start_rejection: rejected(action) }) }));
  assert.ok(trialReplayRejected(action, negative.start_rejection));
});

const require = createRequire(import.meta.url);
const bundled = await bundle({ entryPoints: [fileURLToPath(new URL("rehearsal-panel.jsx", import.meta.url))],
  bundle: true, write: false, format: "esm", platform: "node", loader: { ".css": "empty" }, logLevel: "silent",
  nodePaths: [fileURLToPath(new URL("node_modules/", import.meta.url))],
  alias: { "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)) },
  plugins: [{ name: "shared-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, (args) => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }] });
const { RehearsalPanel, RehearsalSameJobReplay, RehearsalContinuation } = await import(
  `data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);

test("the replay card explains saved data, offline limits and recovery without exposing technical IDs", () => {
  const html = renderToStaticMarkup(React.createElement(RehearsalSameJobReplay, { offer: offer() }));
  assert.match(html, /Try the same job again/); assert.match(html, /original saved job, sample data and follow-up/);
  assert.match(html, /offline trial/); assert.doesNotMatch(html, new RegExp(originalBuild + "|" + oldTrial + "|payload_sha256"));
  assert.equal(renderToStaticMarkup(React.createElement(RehearsalSameJobReplay, {})), "");
});

test("a finished replay is shown only for the exact current saved job with confirmed cleanup", () => {
  const result = completedReplay();
  assert.equal(applicationTrialReplayResult(offer(), result), result);
  for (const changes of [{ current: false }, { status: "running" }, { status: "failed" },
    { worker_stop_confirmed: false }, { application_stop_confirmed: false }, { trace_complete: false },
    { start_request_complete: false }, { guide_version: 8 },
    { rehearsal_id: oldTrial, start_request_id: oldTrial },
    { application_source: { build_id: targetBuild, source_digest: "f".repeat(64) } },
    { trial_replay: { ...result.trial_replay, payload_sha256: "f".repeat(64) } },
    { trial_replay: { ...result.trial_replay, original_rehearsal_id: nextAction } },
    { trial_replay: { ...result.trial_replay, original_source_digest: "f".repeat(64) } },
    { trial_replay: { ...result.trial_replay, status: "failed" } },
    { trial_replay: { ...result.trial_replay, completed_turns: 0 } }])
    assert.equal(applicationTrialReplayResult(offer(), { ...result, ...changes }), null);
  const html = renderToStaticMarkup(React.createElement(RehearsalSameJobReplay, { offer: offer(), receipt: result }));
  assert.match(html, /Saved job replay finished/); assert.match(html, /Review the recorded results below/);
  assert.match(html, /offline trial; the outcome still needs review/);
  assert.match(html, /class="implementation-link"[^>]*>Run the saved job again/);
  assert.doesNotMatch(html, /<h3>Try the same job again|the new outcome|payload_sha256/);
  const pending = createTrialReplayStore(storage()).begin(offer(), nextAction);
  const recovery = renderToStaticMarkup(React.createElement(RehearsalSameJobReplay,
    { offer: offer(), receipt: result, action: pending }));
  assert.match(recovery, /Recover saved retry/); assert.doesNotMatch(recovery, /Saved job replay finished/);
});

async function mounted({ saved = storage(), initialBuild = build, initialState = guide, initialReceipt = oldReceipt, onPost }, verify) {
  const { JSDOM } = createRequire(new URL("../../console/package.json", import.meta.url))("jsdom");
  const dom = new JSDOM('<div id="root"></div>', { url: "http://localhost/", pretendToBeVisual: true });
  const calls = [], acknowledged = [];
  let selectedState = initialState, selectedBuild = initialBuild, latest = initialReceipt, issued = 0;
  const response = (value) => ({ ok: true, status: 202, json: async () => value });
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, sessionStorage: saved, IS_REACT_ACT_ENVIRONMENT: true,
    crypto: { randomUUID: () => [actionId, nextAction][issued++] },
    requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window),
    cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window),
    fetch: async (url, options = {}) => {
      const body = options.body && JSON.parse(options.body);
      if (options.method === "POST") { calls.push({ url, body }); const value = await onPost(body, url);
        return value?.httpStatus ? { ok: false, status: value.httpStatus, json: async () => value.body } : response(value); }
      if (url === "/api/capabilities") return response({ rehearsal: { start_request_replay: true } });
      if (url.includes("/timeline?")) return response({ rehearsal_id: latest?.rehearsal_id, frames: [] });
      return response(latest);
    } };
  const prior = new Map(Object.keys(replacements).map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  for (const [name, value] of Object.entries(replacements)) Object.defineProperty(globalThis, name, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const root = createRoot(dom.window.document.getElementById("root"));
  const mount = async (state = selectedState, candidate = selectedBuild) => {
    selectedState = state; selectedBuild = candidate;
    await act(async () => root.render(React.createElement(RehearsalPanel,
      { state, build: candidate, plan: { flows: [] }, onReceipt(value) { acknowledged.push(value); } })));
  };
  const button = (label) => [...dom.window.document.querySelectorAll("button")].find((node) => node.textContent === label);
  try {
    await mount(); await act(async () => new Promise((resolve) => setTimeout(resolve, 10)));
    await verify({ dom, saved, calls, acknowledged, mount, button,
      click: async (label) => { assert.ok(button(label), label); await act(async () => button(label).click()); },
      setLatest(value) { latest = value; },
      settle: async (operation) => { await act(async () => { operation(); await new Promise((resolve) => setTimeout(resolve, 10)); }); } });
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [name, descriptor] of prior) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor); else delete globalThis[name];
    }
  }
}

test("reloading a completed successor shows its result without automatically dispatching another replay", async () => {
  await mounted({ initialReceipt: completedReplay(), onPost() { assert.fail("Reload must not start another trial"); } }, async (ui) => {
    assert.match(ui.dom.window.document.body.textContent, /Saved job replay finished/);
    assert.equal(ui.button("Try the same job again") === undefined, true);
    assert.equal(ui.button("Run the saved job again").className, "implementation-link");
    assert.equal(ui.button("Run the saved job again").disabled, false);
    assert.equal(ui.calls.length, 0);
    assert.equal(createTrialReplayStore(ui.saved).pending(guide.id), null);
  });
});

test("explicitly running a completed saved job again retains its exact original replay request", async () => {
  const saved = storage();
  await mounted({ saved, initialReceipt: completedReplay(), onPost(body, url) {
    assert.equal(url, `/api/guide/${guide.id}/rehearsal/${oldTrial}/replay`);
    assert.deepEqual(body, { version: guide.version, original_build_id: originalBuild,
      original_source_digest: "a".repeat(64), build_id: targetBuild,
      source_digest: "b".repeat(64), start_request_id: actionId });
    return admitted(createTrialReplayStore(saved).pending(guide.id));
  } }, async (ui) => {
    await ui.click("Run the saved job again");
    assert.equal(ui.calls.length, 1);
    assert.equal(createTrialReplayStore(saved).pending(guide.id), null);
  });
});

test("the mounted wizard retests a historical selected job without reading or changing editable fixtures", async () => {
  const saved = storage(), drafts = createRehearsalDraftStore(saved), owner = rehearsalSetupDraftOwner(guide, build, "application");
  drafts.update(owner, "sample", "My newer unsent job"); drafts.update(owner, "context", "A newer draft document");
  drafts.update(owner, "applicationData", '{"private_draft":"must never be replayed"}'); drafts.selectMode(guide, build, "application");
  await mounted({ saved, onPost(body, url) {
    assert.equal(url, `/api/guide/${guide.id}/rehearsal/${oldTrial}/replay`);
    const action = createTrialReplayStore(saved).pending(guide.id);
    assert.deepEqual(body, action.body); return admitted(action);
  } }, async (ui) => {
    assert.equal(ui.button("Try the same job again").disabled, false);
    assert.equal(ui.calls.length, 0);
    await ui.click("Try the same job again");
    assert.equal(ui.calls.length, 1);
    assert.equal(createTrialReplayStore(saved).pending(guide.id), null);
    assert.equal(createRehearsalDraftStore(saved).read(owner).sample, "My newer unsent job");
    assert.equal(createRehearsalDraftStore(saved).read(owner).context, "A newer draft document");
    assert.equal(createRehearsalDraftStore(saved).read(owner).applicationData, '{"private_draft":"must never be replayed"}');
  });
});

test("a lost replay reply stays pending across reload and source edits, then recovers exactly the original action", async () => {
  const saved = storage(); let first;
  await mounted({ saved, onPost(body) { first = structuredClone(body); throw new Error("Lost retry response"); } }, async (ui) => {
    await ui.click("Try the same job again");
    assert.equal(ui.button("Try agent design").disabled, true);
    assert.ok(ui.button("Recover saved retry"));
    assert.deepEqual(createTrialReplayStore(saved).pending(guide.id).body, first);
  });
  const changedGuide = { ...guide, version: 10 }, changedBuild = { ...build, guide_version: 10,
    source_digest: "f".repeat(64), trial_replay: { available: false } };
  await mounted({ saved, initialState: changedGuide, initialBuild: changedBuild, onPost(body) {
    assert.deepEqual(body, first); return admitted(createTrialReplayStore(saved).pending(guide.id), { current: false });
  } }, async (ui) => {
    assert.ok(ui.button("Recover saved retry"));
    await ui.click("Recover saved retry");
    assert.equal(ui.calls.length, 1); assert.equal(createTrialReplayStore(saved).pending(guide.id), null);
  });
});

test("mounted unbound negative responses keep recovery while a typed rejection permits explicit preserved separation", async () => {
  for (const bound of [false, true]) {
    const saved = storage();
    await mounted({ saved, onPost() {
      const action = createTrialReplayStore(saved).pending(guide.id);
      return { httpStatus: 409, body: { detail: "The host did not admit this retry.",
        start_rejection: rejected(action, bound ? {} : { payload_sha256: "f".repeat(64) }) } };
    } }, async (ui) => {
      await ui.click("Try the same job again");
      assert.ok(ui.button("Recover saved retry"));
      assert.equal(!!ui.button("Keep rejected retry and prepare another"), bound);
      assert.equal(ui.button("Try agent design").disabled, true);
      if (bound) {
        await ui.click("Keep rejected retry and prepare another");
        assert.equal(ui.calls.length, 1); assert.equal(createTrialReplayStore(saved).pending(guide.id), null);
        assert.equal(ui.button("Try the same job again").disabled, false);
        assert.equal(JSON.parse(saved.getItem(key + ":rejected"))[0].body.start_request_id, actionId);
      }
    });
  }
});

test("a source change hides the offered action and another guide cannot recover its saved retry", async () => {
  const saved = storage(); createTrialReplayStore(saved).begin(offer(), actionId);
  await mounted({ saved, initialState: { id: oldTrial, version: 9 }, initialBuild: null,
    onPost() { assert.fail("Another guide cannot replay this action"); } }, async (ui) => {
    assert.equal(ui.button("Try the same job again") === undefined, true);
    assert.equal(ui.button("Recover saved retry") === undefined, true); assert.equal(ui.calls.length, 0);
  });
  await mounted({ initialBuild: { ...build, source_digest: "f".repeat(64) },
    onPost() { assert.fail("Edited unchecked source cannot be offered"); } }, async (ui) => {
    assert.equal(ui.button("Try the same job again") === undefined, true); assert.equal(ui.calls.length, 0);
  });
});

const waitingReplay = (action) => {
  const result = admitted(action);
  return { ...result, status: "waiting_input", keep_open: true, can_continue: true,
    turn_count: 1, current_turn: 1, worker_stop_confirmed: false,
    idle_expires_at: "2099-10-03T03:20:00Z", expires_at: "2099-10-03T03:25:00Z",
    trial_replay: { ...result.trial_replay, expected_turns: 3, completed_turns: 0, status: "replaying" } };
};

test("a saved journey's transient waiting state shows automatic progress and End without manual continuation", () => {
  const action = createTrialReplayStore(storage()).begin(offer(), actionId), receipt = waitingReplay(action);
  const props = { receipt, eligible: true, text: "A preserved ordinary follow-up", onEnd() {} };
  const html = renderToStaticMarkup(React.createElement(RehearsalContinuation, props));
  assert.match(html, /Replaying the saved journey/);
  assert.match(html, /Saved journey progress: 1 of 3 requests have settled/);
  assert.match(html, /next saved request is sent automatically/);
  assert.match(html, /End trial/); assert.match(html, /Trial time limit/);
  assert.doesNotMatch(html, /textarea|Continue this trial|What would you ask next|Follow-up window|preserved ordinary follow-up/);
  const settled = renderToStaticMarkup(React.createElement(RehearsalContinuation,
    { ...props, receipt: { ...receipt, turn_count: 3 } }));
  assert.match(settled, /All saved requests have settled/);
  assert.match(settled, /checking worker cleanup/);
  for (const turn_count of [undefined, "1", -1, 4]) {
    const unknown = renderToStaticMarkup(React.createElement(RehearsalContinuation,
      { ...props, receipt: { ...receipt, turn_count, trial_replay: { ...receipt.trial_replay, completed_turns: undefined } } }));
    assert.doesNotMatch(unknown, /Saved journey progress:|textarea|Continue this trial/);
    assert.match(unknown, /End trial/);
  }
});

test("the mounted replay cannot offer manual continuation, can End, and keeps ordinary follow-up drafts", async () => {
  const saved = storage(), ordinary = { ...oldReceipt, current: true, status: "waiting_input", keep_open: true,
    can_continue: true, turn_count: 1, worker_stop_confirmed: false,
    application_source: { build_id: targetBuild, source_digest: build.source_digest } };
  const ordinaryOwner = rehearsalTurnDraftOwner(ordinary, build, guide);
  const drafts = createRehearsalDraftStore(saved);
  assert.ok(drafts.update(ordinaryOwner, "followUp", "Keep my ordinary follow-up draft."));
  let uiRef, replay;
  await mounted({ saved, onPost(body, url) {
    if (url.endsWith("/replay")) {
      replay = waitingReplay(createTrialReplayStore(saved).pending(guide.id));
      uiRef.setLatest(replay); return replay;
    }
    assert.equal(url, `/api/guide/${guide.id}/rehearsal/end`);
    assert.deepEqual(body, { rehearsal_id: actionId });
    const ending = { ...replay, status: "ending", can_continue: false };
    uiRef.setLatest(ending); return ending;
  } }, async (ui) => {
    uiRef = ui;
    await ui.click("Try the same job again");
    assert.equal(ui.button("Continue this trial") === undefined, true);
    assert.equal(ui.dom.window.document.querySelector('textarea[id^="trial-follow-up-"]') === null, true);
    assert.match(ui.dom.window.document.body.textContent, /1 of 3 requests have settled/);
    assert.match(ui.dom.window.document.body.textContent, /next saved request is sent automatically/);
    assert.doesNotMatch(ui.dom.window.document.body.textContent, /Follow-up window/);
    assert.equal(ui.button("End trial").disabled, false);
    await ui.click("End trial");
    assert.equal(ui.calls.length, 2);
    assert.equal(ui.calls.some((call) => call.url.endsWith("/continue")), false);
    assert.equal(createRehearsalDraftStore(saved).read(ordinaryOwner).followUp, "Keep my ordinary follow-up draft.");
  });
  await mounted({ saved, initialReceipt: ordinary, onPost() { assert.fail("Showing an ordinary draft must not send it"); } }, async (ui) => {
    assert.equal(ui.dom.window.document.querySelector('textarea[id^="trial-follow-up-"]').value, "Keep my ordinary follow-up draft.");
    assert.equal(ui.button("Continue this trial").disabled, false);
    assert.equal(ui.calls.length, 0);
  });
});
